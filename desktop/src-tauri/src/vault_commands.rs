use crate::state::{AppState, AuthGeneration};
use clavyn_core::vault::Vault;
use std::sync::Arc;
use tauri::State;

type ApiResult<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn ensure_vault_uninitialized(vault: &Vault) -> ApiResult<()> {
    if vault.is_initialized() {
        return Err("vault already initialized".into());
    }
    Ok(())
}

/// Commit the passphrase produced by vault initialization only if no newer
/// authentication transition (most importantly a lock) has happened since the
/// initialization request started. A successful commit advances the generation
/// so older unlock attempts cannot later commit against the newly created vault.
fn commit_initialized_passphrase_if_current(
    auth_generation: &AuthGeneration,
    expected_generation: u64,
    slot: &mut Option<zeroize::Zeroizing<String>>,
    passphrase: zeroize::Zeroizing<String>,
) -> bool {
    if !auth_generation.is_current(expected_generation) {
        return false;
    }

    auth_generation.invalidate();
    *slot = Some(passphrase);
    true
}

/// Create a brand-new vault only after proving that no initialized vault is
/// already present. Biometric credentials are bound to the vault generation, so
/// a credential left behind by a reset/downgrade cannot attach to the new vault;
/// the legacy static Keychain account is also cleaned up when available.
///
/// Returns whether the new vault is unlocked. If a newer lock happens while
/// initialization is in progress, creation still succeeds but the vault remains
/// locked instead of resurrecting an unlocked state after that lock.
#[tauri::command]
pub async fn secure_initialize_vault(
    state: State<'_, Arc<AppState>>,
    passphrase: String,
) -> ApiResult<bool> {
    let generation = state.auth_generation.current();
    let passphrase = zeroize::Zeroizing::new(passphrase);
    let mut vault = state.vault.lock().await;

    // This must happen before any Keychain cleanup: a mistaken initialize call
    // must not delete the credential belonging to an existing vault.
    ensure_vault_uninitialized(&vault)?;
    crate::biometric::clear_for_vault_initialization().await;
    vault.initialize(passphrase.as_str()).map_err(err)?;
    drop(vault);

    let mut pw = state.passphrase.lock().await;
    let unlocked = commit_initialized_passphrase_if_current(
        &state.auth_generation,
        generation,
        &mut pw,
        passphrase,
    );
    Ok(unlocked)
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

    let vault = state.vault.lock().await;
    vault.verify_passphrase(passphrase.as_str()).map_err(err)?;
    drop(vault);

    let mut pw = state.passphrase.lock().await;
    if !state.auth_generation.is_current(generation) {
        return Err("vault unlock was superseded by a newer lock".into());
    }
    *pw = Some(passphrase);
    Ok(())
}

/// Lock the vault and invalidate every unlock/initialization attempt that
/// started before this operation. The generation is advanced before waiting for
/// the passphrase mutex so an older operation can never commit after this lock.
#[tauri::command]
pub async fn secure_lock_vault(state: State<'_, Arc<AppState>>) -> ApiResult<()> {
    state.auth_generation.invalidate();
    let mut pw = state.passphrase.lock().await;
    *pw = None;
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
    vault.verify_passphrase(passphrase.as_str()).map_err(err)?;

    // Delete the authoritative vault-bound Keychain item before erasing the
    // binding id needed to address it. If deletion fails, abort with the vault
    // intact so the user can retry safely. Legacy-account cleanup remains
    // best-effort inside clear_for_reset().
    let binding_id = vault
        .binding_id()
        .ok_or_else(|| "vault binding is unavailable".to_string())?
        .to_owned();
    crate::biometric::clear_for_reset(&binding_id).await?;

    vault.reset().map_err(err)?;

    // File deletion is the irreversible reset boundary. Invalidate the auth
    // generation while we still own the vault lock, before an older unlock can
    // make progress and before waiting for the passphrase mutex.
    state.auth_generation.invalidate();
    drop(vault);
    drop(passphrase);

    let mut pw = state.passphrase.lock().await;
    *pw = None;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        commit_initialized_passphrase_if_current, ensure_vault_uninitialized,
    };
    use crate::state::AuthGeneration;
    use clavyn_core::vault::Vault;

    #[test]
    fn rejects_initialization_when_a_vault_already_exists() {
        let path = std::env::temp_dir().join(format!(
            "clavyn-vault-command-test-{}.json",
            uuid::Uuid::new_v4()
        ));
        let mut vault = Vault::open(path.clone()).expect("open vault");

        assert!(ensure_vault_uninitialized(&vault).is_ok());
        vault
            .initialize("correct horse battery staple")
            .expect("initialize vault");
        assert_eq!(
            ensure_vault_uninitialized(&vault).unwrap_err(),
            "vault already initialized"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn newer_lock_prevents_initialization_from_reunlocking_vault() {
        let generation = AuthGeneration::new();
        let expected = generation.current();
        generation.invalidate(); // simulate a newer secure_lock_vault call

        let mut slot = None;
        let committed = commit_initialized_passphrase_if_current(
            &generation,
            expected,
            &mut slot,
            zeroize::Zeroizing::new("correct horse battery staple".to_string()),
        );

        assert!(!committed);
        assert!(slot.is_none());
    }

    #[test]
    fn successful_initialization_commit_starts_a_new_auth_epoch() {
        let generation = AuthGeneration::new();
        let expected = generation.current();
        let mut slot = None;

        let committed = commit_initialized_passphrase_if_current(
            &generation,
            expected,
            &mut slot,
            zeroize::Zeroizing::new("correct horse battery staple".to_string()),
        );

        assert!(committed);
        assert!(slot.is_some());
        assert!(!generation.is_current(expected));
    }

    #[test]
    fn reset_epoch_invalidates_older_authentication_before_slot_cleanup() {
        let generation = AuthGeneration::new();
        let older_attempt = generation.current();
        let mut slot = Some(zeroize::Zeroizing::new("secret".to_string()));

        // Mirrors the reset boundary: generation first, passphrase slot second.
        generation.invalidate();
        assert!(!generation.is_current(older_attempt));
        slot = None;

        assert!(slot.is_none());
    }
}
