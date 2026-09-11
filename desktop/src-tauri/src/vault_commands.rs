use crate::state::{AppState, VaultSession};
use clavyn_core::{vault::Vault, CoreError};
use std::sync::Arc;
use tauri::State;

type ApiResult<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn reset_crossed_destructive_boundary(reset_result: &ApiResult<()>, vault: &Vault) -> bool {
    reset_result.is_ok() || !vault.is_initialized()
}

fn normalize_reset_result(
    reset_result: clavyn_core::Result<()>,
    vault: &Vault,
) -> ApiResult<()> {
    match reset_result {
        Ok(()) => Ok(()),
        Err(error) if !vault.is_initialized() => Err(CoreError::VaultResetDurability(
            error.to_string(),
        )
        .to_string()),
        Err(error) => Err(err(error)),
    }
}

/// Unlock the vault only after proving that the supplied passphrase decrypts
/// the authenticated vault payload. A lock that happens while verification is
/// in flight invalidates this attempt before it can commit the passphrase.
#[tauri::command]
pub async fn secure_unlock_vault(
    state: State<'_, Arc<AppState>>,
    passphrase: String,
) -> ApiResult<()> {
    let generation = state.auth_generation.current();
    let passphrase = zeroize::Zeroizing::new(passphrase);

    let mut vault = state.vault.lock().await;
    // Verification derives the master key; keep it for the unlocked session so
    // later vault operations do not each repeat the Argon2 work.
    let key = vault
        .verify_passphrase(passphrase.as_str())
        .await
        .map_err(err)?;
    let binding_id = vault
        .binding_id()
        .ok_or_else(|| "vault binding is unavailable".to_string())?
        .to_owned();
    // A vault stored in an older on-disk format is rewritten in the
    // authenticated one here, the one moment the master key is available. The
    // rewrite is atomic and preserves the salt, so a failure leaves the file and
    // any bound Keychain credential intact and must not fail the unlock.
    if let Err(error) = vault.migrate_to_current_format(&key) {
        tracing::warn!("vault format upgrade deferred: {error}");
    }
    drop(vault);

    let session = VaultSession::new(passphrase, key, binding_id);
    if !state
        .vault_session
        .unlock_if_current(&state.auth_generation, generation, session)
        .await
    {
        return Err("vault unlock was superseded by a newer lock".into());
    }
    Ok(())
}

/// Lock the vault and invalidate every unlock/initialization attempt that
/// started before this operation. The generation is advanced before waiting for
/// the passphrase mutex so an older operation can never commit after this lock.
#[tauri::command]
pub async fn secure_lock_vault(state: State<'_, Arc<AppState>>) -> ApiResult<()> {
    state.auth_generation.invalidate();
    state.vault_session.clear().await;
    Ok(())
}

/// Permanently destroy the vault after proving knowledge of the current master
/// passphrase. Every encrypted private key and credential is irrecoverably
/// lost, the on-disk vault file is deleted, and in-memory state is reset to
/// uninitialized.
///
/// Authorization is enforced before any destructive action: a wrong passphrase
/// never destroys the vault. The current vault-bound biometric credential is
/// then deleted authoritatively while its binding id still exists. Only after
/// that cleanup succeeds is the vault file destroyed and authentication state
/// invalidated.
#[tauri::command]
pub async fn secure_reset_vault(
    state: State<'_, Arc<AppState>>,
    passphrase: String,
) -> ApiResult<()> {
    let passphrase = zeroize::Zeroizing::new(passphrase);

    // Use the same mutation boundary as biometric enable/disable so no direct
    // IPC caller can recreate the old vault-bound credential between cleanup
    // and destruction.
    let _biometric_mutation = state.biometric_mutation.lock().await;
    let mut vault = state.vault.lock().await;
    if !vault.is_initialized() {
        return Err("vault is not initialized".into());
    }

    // Authorization gate: prove knowledge of the current passphrase before
    // touching the on-disk file or any biometric credential.
    vault
        .verify_passphrase(passphrase.as_str())
        .await
        .map_err(err)?;

    // Delete the authoritative vault-bound Keychain item before erasing the
    // binding id needed to address it. The durable enrollment marker prevents a
    // differently signed macOS build from treating an access-group-invisible
    // credential as already missing. Any uncertainty aborts with the vault intact.
    let binding_id = vault
        .binding_id()
        .ok_or_else(|| "vault binding is unavailable".to_string())?
        .to_owned();
    crate::vault_keychain_cleanup::clear_for_reset(&state.app_data_dir, &binding_id).await?;

    let core_reset_result = vault.reset();
    let reset_result = normalize_reset_result(core_reset_result, &vault);
    if !reset_crossed_destructive_boundary(&reset_result, &vault) {
        return reset_result;
    }

    // Once the authoritative file has been unlinked, invalidate authentication
    // even if the subsequent directory fsync reported a durability error. The
    // process must never keep an in-memory passphrase, or the key derived from
    // it, for a vault that is gone.
    state.auth_generation.invalidate();
    drop(vault);
    drop(passphrase);

    state.vault_session.clear().await;

    reset_result
}

#[cfg(test)]
mod tests {
    use super::{normalize_reset_result, reset_crossed_destructive_boundary};
    use crate::state::{AuthGeneration, VaultSession, VaultSessionSlot};
    use clavyn_core::{vault::Vault, CoreError};

    #[tokio::test]
    async fn reset_epoch_invalidates_older_authentication_before_slot_cleanup() {
        let generation = AuthGeneration::new();
        let older_attempt = generation.current();
        let slot = VaultSessionSlot::new();
        slot.unlock_if_current(
            &generation,
            older_attempt,
            VaultSession::new(
                zeroize::Zeroizing::new("secret".to_string()),
                zeroize::Zeroizing::new([1u8; 32]),
                "binding".to_string(),
            ),
        )
        .await;

        // Mirrors the reset boundary: generation first, credentials second.
        generation.invalidate();
        assert!(!generation.is_current(older_attempt));
        slot.clear().await;

        assert!(!slot.is_unlocked().await);
        assert!(slot.passphrase().await.is_none());
        assert!(slot.key_for("binding").await.is_none());
    }

    #[test]
    fn reset_error_after_destructive_boundary_still_requires_auth_cleanup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
        let reset_result = Err::<(), String>("directory sync failed".into());

        assert!(reset_crossed_destructive_boundary(&reset_result, &vault));
    }

    #[tokio::test]
    async fn reset_error_before_destructive_boundary_keeps_auth_state_retryable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path).expect("open vault");
        vault
            .initialize("correct horse battery staple")
            .await
            .expect("initialize vault");
        let reset_result = Err::<(), String>("unlink failed".into());

        assert!(!reset_crossed_destructive_boundary(&reset_result, &vault));
    }

    #[test]
    fn post_delete_reset_errors_are_tagged_for_frontend_reconciliation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
        let result = normalize_reset_result(
            Err(CoreError::Io(std::io::Error::other("directory sync failed"))),
            &vault,
        );

        let error = result.unwrap_err();
        assert!(error.contains("[vault-reset-durability]"));
    }
}
