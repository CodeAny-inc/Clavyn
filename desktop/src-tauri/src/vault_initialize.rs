use crate::state::{AppState, AuthGeneration};
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

    vault
        .initialize(passphrase.as_str())
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

    let mut pw = state.passphrase.lock().await;
    let unlocked = commit_initialized_passphrase_if_current(
        &state.auth_generation,
        generation,
        &mut pw,
        passphrase,
    );
    Ok(unlocked)
}

#[cfg(test)]
mod tests {
    use super::commit_initialized_passphrase_if_current;
    use crate::state::AuthGeneration;

    #[test]
    fn newer_lock_still_prevents_initialization_commit() {
        let generation = AuthGeneration::new();
        let expected = generation.current();
        generation.invalidate();
        let mut slot = None;
        assert!(!commit_initialized_passphrase_if_current(
            &generation,
            expected,
            &mut slot,
            zeroize::Zeroizing::new("passphrase".to_string()),
        ));
        assert!(slot.is_none());
    }
}
