use crate::state::{AppState, VaultSession};
use clavyn_core::vault::Vault;
use std::sync::Arc;
use tauri::State;

type ApiResult<T> = std::result::Result<T, String>;

async fn blocking_platform_call<T, F>(operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|e| format!("biometric worker failed: {e}"))?
}

/// Keychain identifiers for the stored vault passphrase. Each protected item is
/// additionally bound to the current vault generation so an orphaned Keychain
/// item can never become authoritative for a newly initialized vault.
#[cfg(all(target_os = "macos", feature = "macos-biometric"))]
const SERVICE: &str = "com.clavyn.vault";
#[cfg(all(target_os = "macos", feature = "macos-biometric"))]
const ACCOUNT_PREFIX: &str = "master-passphrase";

fn vault_binding_id(vault: &Vault) -> ApiResult<String> {
    vault
        .binding_id()
        .map(str::to_owned)
        .ok_or_else(|| "vault not initialized".to_string())
}

// ============================================================
// macOS implementation — Touch ID via Security framework
// ============================================================

#[cfg(all(target_os = "macos", feature = "macos-biometric"))]
mod macos {
    use super::{ACCOUNT_PREFIX, SERVICE};
    use objc2_local_authentication::{LAContext, LAPolicy};
    use security_framework::base::Error;
    use security_framework::passwords::{self, PasswordOptions};
    use security_framework_sys::base::{errSecAuthFailed, errSecItemNotFound};

    // Security.framework does not currently expose these through
    // security-framework-sys. Keep the numeric OSStatus values local and map
    // them to explicit, fail-closed errors below.
    const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;
    const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;

    fn account_for_binding(binding_id: &str) -> String {
        format!("{ACCOUNT_PREFIX}:{binding_id}")
    }

    fn password_options_for_account(account: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(SERVICE, account);
        // On macOS, SecItem defaults to the legacy file-based keychain. Access
        // control backed by Touch ID must live in the Data Protection Keychain.
        options.use_protected_keychain();
        options
    }

    fn password_options(binding_id: &str) -> PasswordOptions {
        password_options_for_account(&account_for_binding(binding_id))
    }

    /// Return whether this Mac can currently evaluate a biometric-only policy.
    /// On macOS this corresponds to Touch ID availability/enrollment and also
    /// accounts for temporary states such as biometric lockout.
    pub fn biometry_available() -> bool {
        unsafe {
            let context = LAContext::new();
            context
                .canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
                .is_ok()
        }
    }

    /// Retrieve the passphrase for this exact vault generation from the Data
    /// Protection Keychain. This blocks while macOS presents the Touch ID prompt.
    pub fn retrieve_passphrase(binding_id: &str) -> Result<zeroize::Zeroizing<String>, String> {
        let data = passwords::generic_password(password_options(binding_id))
            .map_err(format_keychain_error)?;
        let s = String::from_utf8(data)
            .map_err(|e| format!("keychain data is not valid UTF-8: {e}"))?;
        Ok(zeroize::Zeroizing::new(s))
    }

    fn format_keychain_error(e: Error) -> String {
        let code = e.code();
        match code {
            errSecItemNotFound => "no biometric passphrase stored".into(),
            errSecAuthFailed => "Touch ID authentication failed or was canceled".into(),
            // errSecUserCanceled
            -128 => "Touch ID was canceled by the user".into(),
            ERR_SEC_INTERACTION_NOT_ALLOWED => {
                "Touch ID interaction is not allowed in the current context".into()
            }
            ERR_SEC_MISSING_ENTITLEMENT => concat!(
                "biometric keychain access requires a properly signed macOS app ",
                "with valid Data Protection Keychain entitlements"
            )
            .into(),
            _ => match e.message() {
                Some(message) => format!("keychain error (code {code}): {message}"),
                None => format!("keychain error (code {code})"),
            },
        }
    }

    #[cfg(test)]
    mod tests {
        use super::account_for_binding;

        #[test]
        fn keychain_account_is_bound_to_vault_generation() {
            assert_eq!(
                account_for_binding("vault-generation"),
                "master-passphrase:vault-generation"
            );
        }
    }
}

// ============================================================
// Stub — biometric unlock unavailable
// ============================================================

// The stub is used on non-macOS platforms and on ordinary/ad-hoc-signed macOS
// builds. It deliberately cannot create or retrieve a protected Keychain item.
// Reset/disable cleanup is handled separately so a tracked credential that is
// invisible under a different macOS signing access group fails closed.
#[cfg(not(all(target_os = "macos", feature = "macos-biometric")))]
mod stub {
    pub fn biometry_available() -> bool {
        false
    }
    pub fn retrieve_passphrase(
        _binding_id: &str,
    ) -> Result<zeroize::Zeroizing<String>, String> {
        Err("biometric unlock is not available in this build".into())
    }
}

#[cfg(all(target_os = "macos", feature = "macos-biometric"))]
use macos as platform;
#[cfg(not(all(target_os = "macos", feature = "macos-biometric")))]
use stub as platform;

/// Best-effort migration cleanup for the static account used by earlier
/// revisions. The independent cleanup module remains compiled on macOS even
/// when Touch ID enrollment/unlock support is disabled.
pub(crate) async fn clear_for_vault_initialization() {
    crate::vault_keychain_cleanup::clear_legacy_best_effort("vault initialization").await;
}

// ============================================================
// Tauri commands
// ============================================================

/// Returns `true` when this build enables biometric support and this Mac can
/// currently evaluate Touch ID. Ordinary/ad-hoc-signed builds return `false`.
#[tauri::command]
pub async fn biometric_available() -> ApiResult<bool> {
    Ok(platform::biometry_available())
}

/// Unlock the vault by retrieving the passphrase from the Keychain item bound
/// to the current vault generation. On an enabled macOS build this triggers the
/// Touch ID prompt. A newer lock invalidates the attempt before commit.
#[tauri::command]
pub async fn unlock_with_biometric(state: State<'_, Arc<AppState>>) -> ApiResult<bool> {
    let generation = state.auth_generation.current();
    let binding_id = {
        let vault = state.vault.lock().await;
        vault_binding_id(&vault)?
    };

    let passphrase = blocking_platform_call({
        let binding_id = binding_id.clone();
        move || platform::retrieve_passphrase(&binding_id)
    })
    .await?;

    let vault = state.vault.lock().await;
    let key = vault
        .verify_passphrase(passphrase.as_str())
        .await
        .map_err(|e| e.to_string())?;
    drop(vault);

    if !state
        .vault_session
        .unlock_if_current(
            &state.auth_generation,
            generation,
            VaultSession::new(passphrase, key, binding_id),
        )
        .await
    {
        return Err("biometric unlock was superseded by a newer vault lock".into());
    }

    Ok(true)
}
