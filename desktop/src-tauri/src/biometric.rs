use crate::state::{AppState, AuthGeneration};
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

fn biometric_enrollment_is_current(
    auth_generation: &AuthGeneration,
    expected_generation: u64,
    vault: &Vault,
    binding_id: &str,
) -> bool {
    auth_generation.is_current(expected_generation)
        && vault.binding_id() == Some(binding_id)
}

// ============================================================
// macOS implementation — Touch ID via Security framework
// ============================================================

#[cfg(all(target_os = "macos", feature = "macos-biometric"))]
mod macos {
    use super::{ACCOUNT_PREFIX, SERVICE};
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_foundation_sys::base::{CFTypeRef, OSStatus};
    use core_foundation_sys::string::CFStringRef;
    use objc2_local_authentication::{LAContext, LAPolicy};
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::base::Error;
    use security_framework::passwords::{self, AccessControlOptions, PasswordOptions};
    use security_framework_sys::base::{errSecAuthFailed, errSecItemNotFound, errSecSuccess};
    use security_framework_sys::item::kSecUseAuthenticationUI;
    use security_framework_sys::keychain_item::SecItemCopyMatching;

    // Security.framework does not currently expose these through
    // security-framework-sys. Keep the numeric OSStatus values local and map
    // them to explicit, fail-closed errors below.
    const ERR_SEC_INTERACTION_NOT_ALLOWED: OSStatus = -25308;
    const ERR_SEC_MISSING_ENTITLEMENT: OSStatus = -34018;

    // `kSecUseAuthenticationUIFail` is deprecated by Apple in favor of an
    // LAContext with interactionNotAllowed, but it remains the most direct way
    // to guarantee this existence-only query never presents authentication UI
    // without bridging an Objective-C LAContext through CoreFoundation FFI.
    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecUseAuthenticationUIFail: CFStringRef;
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PassphraseState {
        Stored,
        Missing,
        Invalidated,
    }

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

    fn classify_passphrase_status(status: OSStatus) -> Result<PassphraseState, String> {
        match status {
            errSecSuccess | ERR_SEC_INTERACTION_NOT_ALLOWED => Ok(PassphraseState::Stored),
            errSecItemNotFound => Ok(PassphraseState::Missing),
            // An item protected with BIOMETRY_CURRENT_SET can become invalid
            // after fingerprints are added or removed. Treat that expected
            // lifecycle state as replaceable rather than making re-enrollment
            // impossible; the command caller has already verified the master
            // passphrase before store_passphrase() is reached.
            errSecAuthFailed => Ok(PassphraseState::Invalidated),
            code => Err(format_keychain_error(Error::from_code(code))),
        }
    }

    /// Probe the protected credential without displaying authentication UI.
    /// A valid biometric item requires interaction and therefore reports as
    /// `Stored`; an item invalidated by enrollment changes is distinguished so
    /// callers can safely offer explicit re-enrollment with the master password.
    fn passphrase_state(binding_id: &str) -> Result<PassphraseState, String> {
        let mut options = password_options(binding_id);

        #[allow(deprecated)]
        unsafe {
            options.query.push((
                CFString::wrap_under_get_rule(kSecUseAuthenticationUI),
                CFString::wrap_under_get_rule(kSecUseAuthenticationUIFail).into_CFType(),
            ));
        }

        #[allow(deprecated)]
        let params = CFDictionary::from_CFType_pairs(&options.query[..]);

        let mut ret: CFTypeRef = std::ptr::null();
        let status: OSStatus =
            unsafe { SecItemCopyMatching(params.as_concrete_TypeRef(), &mut ret) };

        classify_passphrase_status(status)
    }

    /// Check whether a currently usable protected passphrase exists for this
    /// exact vault generation without displaying an authentication prompt.
    pub fn passphrase_stored(binding_id: &str) -> Result<bool, String> {
        Ok(matches!(
            passphrase_state(binding_id)?,
            PassphraseState::Stored
        ))
    }

    /// Store the passphrase in the Data Protection Keychain with biometric
    /// (Touch ID) access control. `BIOMETRY_CURRENT_SET` binds the credential
    /// to the Touch ID enrollment that exists at enable time, while the account
    /// name binds it to the current vault generation.
    pub fn store_passphrase(binding_id: &str, passphrase: &str) -> Result<(), String> {
        match passphrase_state(binding_id)? {
            PassphraseState::Stored => {
                return Err(concat!(
                    "biometric unlock is already enabled; disable it before replacing ",
                    "the protected credential"
                )
                .into());
            }
            PassphraseState::Missing | PassphraseState::Invalidated => {
                // The caller already verified the master passphrase against the
                // vault. Missing and invalidated credentials are therefore safe
                // to clean up before creating a fresh current-set item.
                delete_if_present(binding_id)?;
            }
        }

        // Build the access-control object explicitly instead of using
        // PasswordOptions::set_access_control_options(), which unwraps ACL
        // creation internally. The ThisDeviceOnly accessibility class prevents
        // the raw vault passphrase from migrating to another device via backup.
        let access_control = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
            AccessControlOptions::BIOMETRY_CURRENT_SET.bits(),
        )
        .map_err(|e| {
            format!(
                "failed to create Touch ID access control: {}",
                format_keychain_error(e)
            )
        })?;

        let mut options = password_options(binding_id);
        options.set_access_control(access_control);

        passwords::set_generic_password_options(passphrase.as_bytes(), options)
            .map_err(format_keychain_error)
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

    /// Delete the protected passphrase for this vault generation. Clearing an
    /// already-missing item is intentionally idempotent.
    pub fn clear_passphrase(binding_id: &str) -> Result<(), String> {
        delete_if_present(binding_id)
    }

    /// Remove the pre-binding account used by earlier revisions of this PR.
    /// This is only migration hygiene; new credentials are never stored there.
    pub fn clear_legacy_passphrase() -> Result<(), String> {
        delete_account_if_present(ACCOUNT_PREFIX)
    }

    fn delete_if_present(binding_id: &str) -> Result<(), String> {
        delete_account_if_present(&account_for_binding(binding_id))
    }

    fn delete_account_if_present(account: &str) -> Result<(), String> {
        match passwords::delete_generic_password_options(password_options_for_account(account)) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == errSecItemNotFound => Ok(()),
            Err(e) => Err(format_keychain_error(e)),
        }
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
        use super::{
            account_for_binding, classify_passphrase_status, PassphraseState,
            ERR_SEC_INTERACTION_NOT_ALLOWED,
        };
        use security_framework_sys::base::{errSecAuthFailed, errSecItemNotFound, errSecSuccess};

        #[test]
        fn classifies_invalidated_biometry_as_replaceable() {
            assert_eq!(
                classify_passphrase_status(errSecAuthFailed).unwrap(),
                PassphraseState::Invalidated
            );
            assert_eq!(
                classify_passphrase_status(errSecItemNotFound).unwrap(),
                PassphraseState::Missing
            );
            assert_eq!(
                classify_passphrase_status(errSecSuccess).unwrap(),
                PassphraseState::Stored
            );
            assert_eq!(
                classify_passphrase_status(ERR_SEC_INTERACTION_NOT_ALLOWED).unwrap(),
                PassphraseState::Stored
            );
        }

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
// builds. It deliberately cannot touch the protected Keychain item. Vault-bound
// account names ensure an item left by an earlier feature-enabled build can
// never become authoritative for a newly initialized vault after a downgrade.
#[cfg(not(all(target_os = "macos", feature = "macos-biometric")))]
mod stub {
    pub fn biometry_available() -> bool {
        false
    }
    pub fn passphrase_stored(_binding_id: &str) -> Result<bool, String> {
        Ok(false)
    }
    pub fn store_passphrase(_binding_id: &str, _passphrase: &str) -> Result<(), String> {
        Err("biometric unlock is not available in this build".into())
    }
    pub fn retrieve_passphrase(
        _binding_id: &str,
    ) -> Result<zeroize::Zeroizing<String>, String> {
        Err("biometric unlock is not available in this build".into())
    }
    pub fn clear_passphrase(_binding_id: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn clear_legacy_passphrase() -> Result<(), String> {
        Ok(())
    }
}

#[cfg(all(target_os = "macos", feature = "macos-biometric"))]
use macos as platform;
#[cfg(not(all(target_os = "macos", feature = "macos-biometric")))]
use stub as platform;

fn finish_with_best_effort_legacy_cleanup(
    context: &str,
    result: ApiResult<()>,
) -> ApiResult<()> {
    if let Err(error) = result {
        tracing::warn!("failed to clean legacy biometric credential during {context}: {error}");
    }
    Ok(())
}

/// Best-effort migration cleanup for the static account used by earlier
/// revisions. New vaults are protected primarily by per-vault Keychain account
/// binding, so an orphaned bound item cannot attach to a newly initialized vault.
/// Cleanup failures are intentionally logged rather than returned: the legacy
/// static account is not authoritative for the new vault generation and must not
/// prevent creation of a password-only vault.
pub(crate) async fn clear_for_vault_initialization() {
    let result = blocking_platform_call(platform::clear_legacy_passphrase).await;
    let _ = finish_with_best_effort_legacy_cleanup("vault initialization", result);
}

/// Remove the biometric credential for the current vault generation before the
/// caller destroys that generation. Deleting the vault-bound item is
/// authoritative: if it fails, reset must abort while the binding id is still
/// available for a later retry. Cleanup of the obsolete pre-binding account
/// remains best-effort migration hygiene.
///
/// The caller must serialize this operation with biometric credential mutations
/// using `AppState::biometric_mutation` so a concurrent enrollment cannot recreate
/// the old credential after cleanup but before vault destruction.
pub(crate) async fn clear_for_reset(binding_id: &str) -> ApiResult<()> {
    let bound_id = binding_id.to_owned();
    blocking_platform_call(move || platform::clear_passphrase(&bound_id)).await?;

    let result = blocking_platform_call(platform::clear_legacy_passphrase).await;
    finish_with_best_effort_legacy_cleanup("vault reset", result)
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

/// Checks whether a protected biometric passphrase is stored for the current
/// vault generation without showing an authentication prompt.
#[tauri::command]
pub async fn biometric_passphrase_stored(
    state: State<'_, Arc<AppState>>,
) -> ApiResult<bool> {
    let binding_id = {
        let vault = state.vault.lock().await;
        match vault.binding_id() {
            Some(id) => id.to_owned(),
            None => return Ok(false),
        }
    };

    blocking_platform_call(move || platform::passphrase_stored(&binding_id)).await
}

/// Store the vault passphrase in the OS keychain, protected by Touch ID.
/// The passphrase is verified against the vault before storing to prevent
/// saving an incorrect passphrase. Credential mutations are serialized against
/// destructive reset, and the vault/auth generation is checked again after the
/// blocking Keychain write so a newer lock still fails closed.
#[tauri::command]
pub async fn store_biometric_passphrase(
    state: State<'_, Arc<AppState>>,
    passphrase: String,
) -> ApiResult<()> {
    let _mutation = state.biometric_mutation.lock().await;
    let generation = state.auth_generation.current();
    let passphrase = zeroize::Zeroizing::new(passphrase);

    let binding_id = {
        let vault = state.vault.lock().await;
        vault
            .verify_passphrase(passphrase.as_str())
            .map_err(|e| e.to_string())?;
        vault_binding_id(&vault)?
    };

    let stored_binding_id = binding_id.clone();
    blocking_platform_call(move || {
        platform::store_passphrase(&stored_binding_id, passphrase.as_str())
    })
    .await?;

    let still_current = {
        let vault = state.vault.lock().await;
        biometric_enrollment_is_current(
            &state.auth_generation,
            generation,
            &vault,
            &binding_id,
        )
    };

    if !still_current {
        let stale_binding_id = binding_id.clone();
        if let Err(error) =
            blocking_platform_call(move || platform::clear_passphrase(&stale_binding_id)).await
        {
            tracing::warn!(
                "failed to clean superseded biometric enrollment after auth transition: {error}"
            );
        }
        return Err("biometric enrollment was superseded by a newer vault state".into());
    }

    Ok(())
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

    let passphrase = blocking_platform_call(move || {
        platform::retrieve_passphrase(&binding_id)
    })
    .await?;

    let vault = state.vault.lock().await;
    vault
        .verify_passphrase(passphrase.as_str())
        .map_err(|e| e.to_string())?;
    drop(vault);

    let mut pw = state.passphrase.lock().await;
    if !state.auth_generation.is_current(generation) {
        return Err("biometric unlock was superseded by a newer vault lock".into());
    }
    *pw = Some(passphrase);

    Ok(true)
}

/// Remove the biometric passphrase for the current vault generation. Deleting
/// that bound credential is authoritative; cleanup of the obsolete pre-binding
/// account is best-effort migration hygiene and must not turn success into an
/// apparent disable failure.
#[tauri::command]
pub async fn clear_biometric_passphrase(
    state: State<'_, Arc<AppState>>,
) -> ApiResult<()> {
    let _mutation = state.biometric_mutation.lock().await;
    let binding_id = {
        let vault = state.vault.lock().await;
        vault.binding_id().map(str::to_owned)
    };

    blocking_platform_call(move || {
        if let Some(binding_id) = binding_id {
            platform::clear_passphrase(&binding_id)?;
        }
        finish_with_best_effort_legacy_cleanup(
            "biometric disable",
            platform::clear_legacy_passphrase(),
        )
    })
    .await
}

#[cfg(test)]
mod cleanup_tests {
    use super::{biometric_enrollment_is_current, finish_with_best_effort_legacy_cleanup};
    use crate::state::AuthGeneration;
    use clavyn_core::vault::Vault;

    #[test]
    fn legacy_cleanup_failure_does_not_fail_authoritative_disable() {
        let result = finish_with_best_effort_legacy_cleanup(
            "biometric disable",
            Err("legacy cleanup failed".into()),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn enrollment_is_rejected_after_auth_generation_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path).expect("open vault");
        vault
            .initialize("correct horse battery staple")
            .expect("initialize vault");
        let binding_id = vault.binding_id().expect("binding id").to_string();
        let generation = AuthGeneration::new();
        let expected = generation.current();

        assert!(biometric_enrollment_is_current(
            &generation,
            expected,
            &vault,
            &binding_id,
        ));

        generation.invalidate();
        assert!(!biometric_enrollment_is_current(
            &generation,
            expected,
            &vault,
            &binding_id,
        ));
    }

    #[test]
    fn enrollment_is_rejected_after_vault_generation_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let mut vault = Vault::open(path).expect("open vault");
        vault
            .initialize("first passphrase")
            .expect("initialize first vault");
        let first_binding = vault.binding_id().expect("first binding").to_string();
        let generation = AuthGeneration::new();
        let expected = generation.current();

        vault.reset().expect("reset vault");
        vault
            .initialize("second passphrase")
            .expect("initialize second vault");

        assert!(!biometric_enrollment_is_current(
            &generation,
            expected,
            &vault,
            &first_binding,
        ));
    }
}
