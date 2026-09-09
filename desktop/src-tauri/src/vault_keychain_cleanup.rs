type ApiResult<T> = std::result::Result<T, String>;

#[cfg(target_os = "macos")]
mod macos {
    use security_framework::passwords::{self, PasswordOptions};
    use security_framework_sys::base::errSecItemNotFound;

    const SERVICE: &str = "com.clavyn.vault";
    const ACCOUNT_PREFIX: &str = "master-passphrase";

    fn account_for_binding(binding_id: &str) -> String {
        format!("{ACCOUNT_PREFIX}:{binding_id}")
    }

    fn password_options_for_account(account: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(SERVICE, account);
        // Biometric credentials are stored in the Data Protection Keychain.
        // Reset cleanup must address the same keychain even when Touch ID
        // enrollment/unlock support is compiled out of the current build.
        options.use_protected_keychain();
        options
    }

    fn delete_account_if_present(account: &str) -> Result<(), String> {
        match passwords::delete_generic_password_options(password_options_for_account(account)) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(error) => {
                let code = error.code();
                match error.message() {
                    Some(message) => Err(format!("keychain error (code {code}): {message}")),
                    None => Err(format!("keychain error (code {code})")),
                }
            }
        }
    }

    pub fn clear_bound_passphrase(binding_id: &str) -> Result<(), String> {
        delete_account_if_present(&account_for_binding(binding_id))
    }

    pub fn clear_legacy_passphrase() -> Result<(), String> {
        delete_account_if_present(ACCOUNT_PREFIX)
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

/// Remove the credential bound to the current vault generation before reset.
/// On macOS this path is compiled independently of `macos-biometric`, allowing
/// an ordinary/ad-hoc-signed build to clean credentials created by an earlier
/// biometric-enabled build. Bound-item deletion is authoritative; obsolete
/// legacy-account cleanup remains best-effort migration hygiene.
#[cfg(target_os = "macos")]
pub(crate) async fn clear_for_reset(binding_id: &str) -> ApiResult<()> {
    let bound_id = binding_id.to_owned();
    blocking_keychain_call(move || macos::clear_bound_passphrase(&bound_id)).await?;

    if let Err(error) = blocking_keychain_call(macos::clear_legacy_passphrase).await {
        tracing::warn!("failed to clean legacy biometric credential during vault reset: {error}");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) async fn clear_for_reset(_binding_id: &str) -> ApiResult<()> {
    Ok(())
}
