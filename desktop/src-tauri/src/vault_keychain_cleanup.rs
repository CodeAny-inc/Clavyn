use std::io::Write;
use std::path::{Path, PathBuf};

type ApiResult<T> = std::result::Result<T, String>;

const ENROLLMENT_MARKER: &str = "vault-biometric-binding";

fn marker_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(ENROLLMENT_MARKER)
}

fn marker_temp_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(format!("{ENROLLMENT_MARKER}.tmp"))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Persist a non-secret vault-binding marker before creating a protected
/// Data Protection Keychain item. The marker is security-relevant even though
/// the binding id itself is not secret: a later build can use it to distinguish
/// "no biometric credential was enrolled" from "a credential may exist in a
/// Keychain access group this build cannot see".
pub(crate) fn record_enrollment_marker(app_data_dir: &Path, binding_id: &str) -> ApiResult<()> {
    std::fs::create_dir_all(app_data_dir)
        .map_err(|error| format!("failed to create biometric marker directory: {error}"))?;

    let path = marker_path(app_data_dir);
    let tmp = marker_temp_path(app_data_dir);
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = options.open(&tmp)?;
        file.write_all(binding_id.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        sync_directory(app_data_dir)?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("failed to persist biometric enrollment marker: {error}"));
    }
    Ok(())
}

fn read_enrollment_marker(app_data_dir: &Path) -> ApiResult<Option<String>> {
    let path = marker_path(app_data_dir);
    match std::fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read biometric enrollment marker: {error}")),
    }
}

/// Return whether the marker belongs to this vault generation. A marker for a
/// different generation is intentionally an error rather than "false": it means
/// a protected credential may already be orphaned and automatic reset/enrollment
/// must fail closed instead of erasing the only cleanup reference.
pub(crate) fn marker_tracks_binding(app_data_dir: &Path, binding_id: &str) -> ApiResult<bool> {
    match read_enrollment_marker(app_data_dir)? {
        None => Ok(false),
        Some(marker) if marker == binding_id => Ok(true),
        Some(_) => Err(concat!(
            "a biometric credential marker exists for a different vault generation; ",
            "clean up the previous credential before continuing"
        )
        .into()),
    }
}

pub(crate) fn clear_enrollment_marker(app_data_dir: &Path, binding_id: &str) -> ApiResult<()> {
    if !marker_tracks_binding(app_data_dir, binding_id)? {
        return Ok(());
    }

    let path = marker_path(app_data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => sync_directory(app_data_dir)
            .map_err(|error| format!("failed to sync biometric marker deletion: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("failed to remove biometric enrollment marker: {error}")),
    }
    Ok(())
}

fn ensure_tracked_delete_is_visible(tracked: bool, missing: bool) -> ApiResult<()> {
    if tracked && missing {
        return Err(concat!(
            "tracked biometric credential is not visible to this macOS build; ",
            "refusing to destroy the vault because the current code-signing ",
            "identity may not have the original Data Protection Keychain access group"
        )
        .into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use security_framework::passwords::{self, PasswordOptions};
    use security_framework_sys::base::errSecItemNotFound;

    const SERVICE: &str = "com.clavyn.vault";
    const ACCOUNT_PREFIX: &str = "master-passphrase";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DeleteOutcome {
        Deleted,
        Missing,
    }

    fn account_for_binding(binding_id: &str) -> String {
        format!("{ACCOUNT_PREFIX}:{binding_id}")
    }

    fn password_options_for_account(account: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(SERVICE, account);
        // Biometric credentials are stored in the Data Protection Keychain.
        // Using the same Keychain API is necessary across build variants, but
        // visibility still depends on the host app's code-signing access groups.
        options.use_protected_keychain();
        options
    }

    fn delete_account(account: &str) -> Result<DeleteOutcome, String> {
        match passwords::delete_generic_password_options(password_options_for_account(account)) {
            Ok(()) => Ok(DeleteOutcome::Deleted),
            Err(error) if error.code() == errSecItemNotFound => Ok(DeleteOutcome::Missing),
            Err(error) => {
                let code = error.code();
                match error.message() {
                    Some(message) => Err(format!("keychain error (code {code}): {message}")),
                    None => Err(format!("keychain error (code {code})")),
                }
            }
        }
    }

    pub fn clear_bound_passphrase(binding_id: &str) -> Result<DeleteOutcome, String> {
        delete_account(&account_for_binding(binding_id))
    }

    pub fn clear_legacy_passphrase() -> Result<(), String> {
        delete_account(ACCOUNT_PREFIX).map(|_| ())
    }

    #[cfg(test)]
    mod tests {
        use super::account_for_binding;

        #[test]
        fn reset_cleanup_uses_the_vault_bound_account() {
            assert_eq!(
                account_for_binding("vault-generation"),
                "master-passphrase:vault-generation"
            );
        }
    }
}

#[cfg(target_os = "macos")]
async fn blocking_keychain_call<T, F>(operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| format!("keychain cleanup worker failed: {error}"))?
}

/// Authoritatively delete the vault-bound protected credential. If a durable
/// enrollment marker says the credential should exist but this build receives
/// `errSecItemNotFound`, do not assume it is gone: a differently signed macOS
/// build may simply lack the original Data Protection Keychain access group.
/// Keeping the marker and vault intact makes that downgrade fail closed.
#[cfg(target_os = "macos")]
pub(crate) async fn clear_bound_credential(
    app_data_dir: &Path,
    binding_id: &str,
) -> ApiResult<()> {
    let tracked = marker_tracks_binding(app_data_dir, binding_id)?;
    let bound_id = binding_id.to_owned();
    let outcome = blocking_keychain_call(move || macos::clear_bound_passphrase(&bound_id)).await?;

    ensure_tracked_delete_is_visible(tracked, outcome == macos::DeleteOutcome::Missing)?;

    if tracked {
        clear_enrollment_marker(app_data_dir, binding_id)?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) async fn clear_bound_credential(
    app_data_dir: &Path,
    binding_id: &str,
) -> ApiResult<()> {
    clear_enrollment_marker(app_data_dir, binding_id)
}

#[cfg(target_os = "macos")]
pub(crate) async fn clear_legacy_best_effort(context: &str) {
    if let Err(error) = blocking_keychain_call(macos::clear_legacy_passphrase).await {
        tracing::warn!("failed to clean legacy biometric credential during {context}: {error}");
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) async fn clear_legacy_best_effort(_context: &str) {}

/// Remove the credential bound to the current vault generation before reset.
/// The durable marker makes cleanup safe across build variants: a build that
/// cannot see a credential known to have been enrolled aborts instead of
/// interpreting access-group mismatch as an already-missing item.
pub(crate) async fn clear_for_reset(app_data_dir: &Path, binding_id: &str) -> ApiResult<()> {
    clear_bound_credential(app_data_dir, binding_id).await?;
    clear_legacy_best_effort("vault reset").await;
    Ok(())
}

#[cfg(test)]
mod marker_tests {
    use super::{
        clear_enrollment_marker, ensure_tracked_delete_is_visible, marker_tracks_binding,
        record_enrollment_marker,
    };

    #[test]
    fn enrollment_marker_tracks_and_clears_exact_vault_generation() {
        let dir = tempfile::tempdir().expect("tempdir");

        assert!(!marker_tracks_binding(dir.path(), "generation-a").unwrap());
        record_enrollment_marker(dir.path(), "generation-a").expect("record marker");
        assert!(marker_tracks_binding(dir.path(), "generation-a").unwrap());
        assert!(marker_tracks_binding(dir.path(), "generation-b").is_err());

        clear_enrollment_marker(dir.path(), "generation-a").expect("clear marker");
        assert!(!marker_tracks_binding(dir.path(), "generation-a").unwrap());
    }

    #[test]
    fn tracked_but_invisible_credential_fails_closed() {
        assert!(ensure_tracked_delete_is_visible(true, true).is_err());
        assert!(ensure_tracked_delete_is_visible(true, false).is_ok());
        assert!(ensure_tracked_delete_is_visible(false, true).is_ok());
    }
}
