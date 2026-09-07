/// Migration-only cleanup for the static Keychain credential created by early
/// revisions of the macOS Touch ID work before it moved to the Data Protection
/// Keychain and per-vault account binding.
///
/// This module deliberately exposes deletion only. It must never become a
/// fallback read/store path for the legacy file-based Keychain item.
#[cfg(all(target_os = "macos", feature = "macos-biometric"))]
fn delete_file_based_legacy_passphrase() -> Result<(), String> {
    use security_framework::passwords;
    use security_framework_sys::base::errSecItemNotFound;

    const SERVICE: &str = "com.opentermius.vault";
    const ACCOUNT: &str = "master-passphrase";

    // Intentionally use the default PasswordOptions path here rather than
    // `use_protected_keychain()`: the earliest implementation stored this
    // static account in the legacy file-based macOS Keychain.
    match passwords::delete_generic_password(SERVICE, ACCOUNT) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == errSecItemNotFound => Ok(()),
        Err(error) => {
            let code = error.code();
            let detail = error
                .message()
                .map(|message| format!(": {message}"))
                .unwrap_or_default();
            Err(format!(
                "legacy file-based biometric credential cleanup failed (code {code}){detail}"
            ))
        }
    }
}

#[cfg(not(all(target_os = "macos", feature = "macos-biometric")))]
fn delete_file_based_legacy_passphrase() -> Result<(), String> {
    Ok(())
}

/// Remove the pre-Data-Protection static credential opportunistically. Failure
/// is non-fatal because the current implementation never reads this account and
/// current vault credentials are bound to a different per-vault account name.
pub fn clear_file_based_legacy_passphrase_best_effort() {
    if let Err(error) = delete_file_based_legacy_passphrase() {
        tracing::warn!("{error}");
    }
}
