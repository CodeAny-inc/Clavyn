use crate::state::{AppState, VaultSession};
use clavyn_core::vault::Vault;
use std::sync::Arc;
use tauri::State;

type ApiResult<T> = std::result::Result<T, String>;

fn ensure_vault_uninitialized(vault: &Vault) -> ApiResult<()> {
    if vault.is_initialized() {
        return Err("vault already initialized".into());
    }
    Ok(())
}

/// Marker-aware initialization path. A newly created vault generation gets an
/// explicit durable `Clear` biometric-tracking state before the command reports
/// success, so it can never be confused with a pre-marker legacy vault after a
/// future signing/access-group change.
#[tauri::command]
pub async fn secure_initialize_vault(
    state: State<'_, Arc<AppState>>,
    passphrase: String,
) -> ApiResult<bool> {
    let generation = state.auth_generation.current();
    let passphrase = zeroize::Zeroizing::new(passphrase);

    // Serialize initialization's tracking cleanup with biometric mutations and
    // reset. Direct IPC callers therefore cannot race stale marker cleanup with
    // a credential operation from the previous vault generation.
    let _biometric_mutation = state.biometric_mutation.lock().await;
    let mut vault = state.vault.lock().await;
    ensure_vault_uninitialized(&vault)?;

    crate::vault_keychain_cleanup::prepare_for_new_vault(&state.app_data_dir).await?;
    crate::biometric::clear_for_vault_initialization().await;

    // Initialization derives the master key; keep it for the session it unlocks
    // instead of paying for a second Argon2 pass on the first vault operation.
    let key = vault
        .initialize(passphrase.as_str())
        .await
        .map_err(|error| error.to_string())?;
    let binding_id = vault
        .binding_id()
        .ok_or_else(|| "new vault binding is unavailable".to_string())?
        .to_owned();

    if let Err(tracking_error) = crate::vault_keychain_cleanup::initialize_tracking_for_new_vault(
        &state.app_data_dir,
        &binding_id,
    ) {
        // The brand-new vault contains no user keys yet. If its mandatory
        // tracking metadata cannot be persisted, roll it back rather than
        // leaving a generation that future reset code must treat as legacy-unknown.
        let rollback = vault.reset();
        return match rollback {
            Ok(()) => Err(format!(
                "failed to initialize biometric tracking; new vault was rolled back: {tracking_error}"
            )),
            Err(rollback_error) => Err(format!(
                "failed to initialize biometric tracking ({tracking_error}); vault rollback also failed: {rollback_error}"
            )),
        };
    }

    drop(vault);
    drop(_biometric_mutation);

    let unlocked = state
        .vault_session
        .unlock_new_vault_if_current(
            &state.auth_generation,
            generation,
            VaultSession::new(passphrase, key, binding_id),
        )
        .await;
    Ok(unlocked)
}

#[cfg(test)]
mod tests {
    use crate::state::{AuthGeneration, VaultSession, VaultSessionSlot};

    #[tokio::test]
    async fn newer_lock_still_prevents_initialization_commit() {
        let generation = AuthGeneration::new();
        let expected = generation.current();
        generation.invalidate();
        let slot = VaultSessionSlot::new();

        assert!(
            !slot
                .unlock_new_vault_if_current(
                    &generation,
                    expected,
                    VaultSession::new(
                        zeroize::Zeroizing::new("passphrase".to_string()),
                        zeroize::Zeroizing::new([0u8; 32]),
                        "binding".to_string(),
                    ),
                )
                .await
        );
        assert!(!slot.is_unlocked().await);
        assert!(slot.key_for("binding").await.is_none());
    }
}
