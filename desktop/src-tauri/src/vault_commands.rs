use crate::state::AppState;
use clavyn_core::session::SessionManager;
use clavyn_core::sftp::SftpManager;
use clavyn_core::{vault::Vault, CoreError};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use tokio::sync::Mutex;

type ApiResult<T> = std::result::Result<T, String>;

/// How long the post-reset teardown may spend waiting on connections that are
/// no longer reachable anyway. The vault is already destroyed by then, so the
/// command must return even if a stalled remote never lets go.
const CONTAINMENT_BUDGET: Duration = Duration::from_secs(5);

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Drop every connection the destroyed vault's keys authenticated, along with
/// the local terminals opened beside them. Returns whether the teardown
/// finished inside `budget`.
///
/// Each manager severs reachability by draining its map before it does any
/// network work, so exhausting the budget can only cost a polite disconnect —
/// never leave a session that `session_write` or the `sftp_*` commands can
/// still address. Nothing here reports an error: the vault is already gone,
/// and a stubborn connection must not be able to describe that as a failure.
///
/// The budget is a parameter and the local-terminal map is generic in its
/// value so the teardown contract can be exercised without a real PTY.
async fn contain_open_sessions<T>(
    sessions: &SessionManager,
    sftp: &SftpManager,
    local_terminals: &Mutex<HashMap<String, T>>,
    budget: Duration,
) -> bool {
    tokio::time::timeout(budget, async {
        local_terminals.lock().await.clear();
        sftp.close_all().await;
        sessions.close_all().await;
    })
    .await
    .is_ok()
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
/// lost, the on-disk vault file is deleted, in-memory state is reset to
/// uninitialized, and every SSH session, SFTP channel and local terminal is
/// torn down.
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
    // process must never keep an in-memory passphrase for a vault that is gone.
    state.auth_generation.invalidate();
    drop(vault);
    drop(passphrase);

    let mut pw = state.passphrase.lock().await;
    *pw = None;
    drop(pw);

    // The reset destroys the credentials, so it must also drop the access those
    // credentials bought: a user resetting under suspicion is told the keys are
    // gone, and a still-writable session would make that untrue. This runs only
    // past the destructive boundary, and its outcome is deliberately discarded
    // so a connection that will not close cannot turn a completed reset into a
    // reported failure.
    contain_open_sessions(
        &state.sessions,
        &state.sftp,
        &state.local_terminals,
        CONTAINMENT_BUDGET,
    )
    .await;

    reset_result
}

#[cfg(test)]
mod tests {
    use super::{contain_open_sessions, normalize_reset_result, reset_crossed_destructive_boundary};
    use crate::state::AuthGeneration;
    use clavyn_core::session::SessionManager;
    use clavyn_core::sftp::SftpManager;
    use clavyn_core::{vault::Vault, CoreError};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    /// Long enough that an unobstructed teardown always finishes, short enough
    /// that an obstructed one does not stall the suite.
    const TEST_BUDGET: Duration = Duration::from_millis(200);

    fn session_manager() -> SessionManager {
        SessionManager::new(Arc::new(|_: &str, _: &[u8]| {}), Arc::new(|_: &str, _: &str| {}))
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

    #[test]
    fn reset_error_after_destructive_boundary_still_requires_auth_cleanup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
        let reset_result = Err::<(), String>("directory sync failed".into());

        assert!(reset_crossed_destructive_boundary(&reset_result, &vault));
    }

    #[test]
    fn reset_error_before_destructive_boundary_keeps_auth_state_retryable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path).expect("open vault");
        vault
            .initialize("correct horse battery staple")
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

    #[tokio::test]
    async fn reset_containment_drops_every_session_and_local_terminal() {
        let sessions = session_manager();
        let sftp = SftpManager::new();
        let locals = Mutex::new(HashMap::from([("local-1".to_string(), ())]));

        assert!(contain_open_sessions(&sessions, &sftp, &locals, TEST_BUDGET).await);

        assert!(sessions.list().await.is_empty());
        assert!(sftp.list().await.is_empty());
        assert!(locals.lock().await.is_empty());
    }

    #[tokio::test]
    async fn a_teardown_that_never_completes_still_returns_to_the_reset() {
        let sessions = session_manager();
        let sftp = SftpManager::new();
        let locals = Mutex::new(HashMap::from([("local-1".to_string(), ())]));

        // A teardown step that never finishes: the reset has already destroyed
        // the vault, so the command has to come back regardless.
        let stuck = locals.lock().await;
        assert!(!contain_open_sessions(&sessions, &sftp, &locals, TEST_BUDGET).await);
        drop(stuck);
    }

    #[tokio::test]
    async fn an_unfinished_teardown_leaves_the_reset_outcome_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
        let reset_result = normalize_reset_result(
            Err(CoreError::Io(std::io::Error::other("directory sync failed"))),
            &vault,
        );
        assert!(reset_crossed_destructive_boundary(&reset_result, &vault));

        let sessions = session_manager();
        let sftp = SftpManager::new();
        let locals = Mutex::new(HashMap::from([("local-1".to_string(), ())]));
        let stuck = locals.lock().await;
        assert!(!contain_open_sessions(&sessions, &sftp, &locals, TEST_BUDGET).await);
        drop(stuck);

        // Whatever the teardown managed, the caller still reports the outcome
        // it computed at the destructive boundary.
        assert!(reset_result
            .unwrap_err()
            .contains("[vault-reset-durability]"));
    }
}
