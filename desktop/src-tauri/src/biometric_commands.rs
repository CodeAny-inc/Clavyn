use crate::state::{AppState, AuthGeneration};
use crate::vault_keychain_cleanup::CredentialObservation;
use clavyn_core::vault::Vault;
use std::sync::Arc;
use tauri::State;

type ApiResult<T> = std::result::Result<T, String>;

/// Only the Touch ID platform module distinguishes a stored credential from an
/// invalidated one; every other build resolves to the stub that reports
/// `Missing`, so those two variants are constructed on macOS alone.
#[cfg_attr(
    not(all(target_os = "macos", feature = "macos-biometric")),
    allow(dead_code)
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialState {
    Stored,
    Missing,
    Invalidated,
}

impl From<CredentialState> for CredentialObservation {
    fn from(value: CredentialState) -> Self {
        match value {
            CredentialState::Stored => CredentialObservation::Stored,
            CredentialState::Missing => CredentialObservation::Missing,
            CredentialState::Invalidated => CredentialObservation::Invalidated,
        }
    }
}

async fn blocking_platform_call<T, F>(operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| format!("biometric worker failed: {error}"))?
}

fn vault_binding_id(vault: &Vault) -> ApiResult<String> {
    vault
        .binding_id()
        .map(str::to_owned)
        .ok_or_else(|| "vault not initialized".to_string())
}

fn enrollment_is_current(
    auth_generation: &AuthGeneration,
    expected_generation: u64,
    vault: &Vault,
    binding_id: &str,
) -> bool {
    auth_generation.is_current(expected_generation)
        && vault.binding_id() == Some(binding_id)
}

#[cfg(all(target_os = "macos", feature = "macos-biometric"))]
mod platform {
    use super::CredentialState;
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_foundation_sys::base::{CFTypeRef, OSStatus};
    use core_foundation_sys::string::CFStringRef;
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::base::Error;
    use security_framework::passwords::{self, AccessControlOptions, PasswordOptions};
    use security_framework_sys::base::{errSecAuthFailed, errSecItemNotFound, errSecSuccess};
    use security_framework_sys::item::kSecUseAuthenticationUI;
    use security_framework_sys::keychain_item::SecItemCopyMatching;

    const SERVICE: &str = "com.clavyn.vault";
    const ACCOUNT_PREFIX: &str = "master-passphrase";
    const ERR_SEC_INTERACTION_NOT_ALLOWED: OSStatus = -25308;
    const ERR_SEC_MISSING_ENTITLEMENT: OSStatus = -34018;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecUseAuthenticationUIFail: CFStringRef;
    }

    fn account_for_binding(binding_id: &str) -> String {
        format!("{ACCOUNT_PREFIX}:{binding_id}")
    }

    fn password_options(binding_id: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(
            SERVICE,
            &account_for_binding(binding_id),
        );
        options.use_protected_keychain();
        options
    }

    fn format_keychain_error(error: Error) -> String {
        let code = error.code();
        match code {
            errSecItemNotFound => "no biometric passphrase stored".into(),
            errSecAuthFailed => "Touch ID authentication failed or was canceled".into(),
            -128 => "Touch ID was canceled by the user".into(),
            ERR_SEC_INTERACTION_NOT_ALLOWED => {
                "Touch ID interaction is not allowed in the current context".into()
            }
            ERR_SEC_MISSING_ENTITLEMENT => concat!(
                "biometric keychain access requires a properly signed macOS app ",
                "with valid Data Protection Keychain entitlements"
            )
            .into(),
            _ => match error.message() {
                Some(message) => format!("keychain error (code {code}): {message}"),
                None => format!("keychain error (code {code})"),
            },
        }
    }

    pub fn credential_state(binding_id: &str) -> Result<CredentialState, String> {
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
        match status {
            errSecSuccess | ERR_SEC_INTERACTION_NOT_ALLOWED => Ok(CredentialState::Stored),
            errSecItemNotFound => Ok(CredentialState::Missing),
            errSecAuthFailed => Ok(CredentialState::Invalidated),
            code => Err(format_keychain_error(Error::from_code(code))),
        }
    }

    pub fn store_passphrase(binding_id: &str, passphrase: &str) -> Result<(), String> {
        match credential_state(binding_id)? {
            CredentialState::Stored => {
                return Err("biometric unlock is already enabled".into());
            }
            CredentialState::Missing | CredentialState::Invalidated => {}
        }

        let access_control = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
            AccessControlOptions::BIOMETRY_CURRENT_SET.bits(),
        )
        .map_err(|error| {
            format!(
                "failed to create Touch ID access control: {}",
                format_keychain_error(error)
            )
        })?;

        let mut options = password_options(binding_id);
        options.set_access_control(access_control);
        passwords::set_generic_password_options(passphrase.as_bytes(), options)
            .map_err(format_keychain_error)
    }
}

#[cfg(not(all(target_os = "macos", feature = "macos-biometric")))]
mod platform {
    use super::CredentialState;

    pub fn credential_state(_binding_id: &str) -> Result<CredentialState, String> {
        Ok(CredentialState::Missing)
    }

    pub fn store_passphrase(_binding_id: &str, _passphrase: &str) -> Result<(), String> {
        Err("biometric unlock is not available in this build".into())
    }
}

async fn probe_credential(binding_id: &str) -> ApiResult<CredentialState> {
    let probe_binding = binding_id.to_owned();
    blocking_platform_call(move || platform::credential_state(&probe_binding)).await
}

/// Existence-only biometric status probe. The migration side effect is now
/// serialized with reset/enable/disable, and the vault binding is revalidated
/// after the blocking Keychain call before any durable tracking state is changed.
#[tauri::command]
pub async fn biometric_passphrase_stored(
    state: State<'_, Arc<AppState>>,
) -> ApiResult<bool> {
    let _mutation = state.biometric_mutation.lock().await;
    let binding_id = {
        let vault = state.vault.lock().await;
        match vault.binding_id() {
            Some(id) => id.to_owned(),
            None => return Ok(false),
        }
    };

    let observation = probe_credential(&binding_id).await?;

    // Even though reset is serialized by the mutation lock, keep an explicit
    // binding check so future vault transitions cannot accidentally reintroduce
    // the stale-marker race through a different path.
    {
        let vault = state.vault.lock().await;
        if vault.binding_id() != Some(binding_id.as_str()) {
            return Err("biometric status probe was superseded by a newer vault generation".into());
        }
    }

    crate::vault_keychain_cleanup::reconcile_credential_observation(
        &state.app_data_dir,
        &binding_id,
        observation.into(),
    )
    .await
}

/// Store the current master passphrase behind Touch ID using a crash-recoverable
/// two-phase tracking protocol. `Pending` is persisted before the Keychain write;
/// a scope probe proves access-group continuity so a restart can safely resolve
/// Pending+Missing as an interrupted enrollment, while a changed signing group
/// still fails closed.
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
            .map_err(|error| error.to_string())?;
        vault_binding_id(&vault)?
    };

    let observation = probe_credential(&binding_id).await?;
    crate::vault_keychain_cleanup::reconcile_credential_observation(
        &state.app_data_dir,
        &binding_id,
        observation.into(),
    )
    .await?;

    match observation {
        CredentialState::Stored => {
            return Err(concat!(
                "biometric unlock is already enabled; disable it before replacing ",
                "the protected credential"
            )
            .into());
        }
        CredentialState::Invalidated => {
            // The invalidated item still exists. Delete it authoritatively before
            // starting a fresh enrollment so the pending state describes only the
            // new credential attempt.
            crate::vault_keychain_cleanup::clear_bound_credential(
                &state.app_data_dir,
                &binding_id,
                false,
            )
            .await?;
        }
        CredentialState::Missing => {}
    }

    crate::vault_keychain_cleanup::begin_enrollment(&state.app_data_dir, &binding_id).await?;

    let stored_binding = binding_id.clone();
    let store_result = blocking_platform_call(move || {
        platform::store_passphrase(&stored_binding, passphrase.as_str())
    })
    .await;

    if let Err(error) = store_result {
        // Reconcile the pending state with the actual Keychain outcome. If the
        // probe itself fails, keep Pending: the next status/enable/disable/reset
        // can recover it without falsely claiming the credential is absent.
        if let Ok(observation) = probe_credential(&binding_id).await {
            if let Err(reconcile_error) =
                crate::vault_keychain_cleanup::reconcile_credential_observation(
                    &state.app_data_dir,
                    &binding_id,
                    observation.into(),
                )
                .await
            {
                tracing::warn!(
                    "failed to reconcile biometric tracking after store failure: {reconcile_error}"
                );
            }
        }
        return Err(error);
    }

    let still_current = {
        let vault = state.vault.lock().await;
        enrollment_is_current(
            &state.auth_generation,
            generation,
            &vault,
            &binding_id,
        )
    };

    if !still_current {
        if let Err(error) = crate::vault_keychain_cleanup::clear_bound_credential(
            &state.app_data_dir,
            &binding_id,
            false,
        )
        .await
        {
            tracing::warn!(
                "failed to clean superseded biometric enrollment after auth transition: {error}"
            );
        }
        return Err("biometric enrollment was superseded by a newer vault state".into());
    }

    // If the process crashes before this transition, Pending+Stored is repaired
    // to Enrolled by the next serialized status probe.
    crate::vault_keychain_cleanup::finish_enrollment(&state.app_data_dir, &binding_id).await
}

#[tauri::command]
pub async fn clear_biometric_passphrase(
    state: State<'_, Arc<AppState>>,
) -> ApiResult<()> {
    let _mutation = state.biometric_mutation.lock().await;
    let binding_id = {
        let vault = state.vault.lock().await;
        vault.binding_id().map(str::to_owned)
    };

    if let Some(binding_id) = binding_id {
        crate::vault_keychain_cleanup::clear_bound_credential(
            &state.app_data_dir,
            &binding_id,
            false,
        )
        .await?;
    }
    crate::vault_keychain_cleanup::clear_legacy_best_effort("biometric disable").await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{enrollment_is_current, CredentialState};
    use crate::state::AuthGeneration;
    use clavyn_core::vault::Vault;

    #[test]
    fn credential_states_keep_missing_distinct_from_invalidated() {
        assert_ne!(CredentialState::Missing, CredentialState::Invalidated);
    }

    #[test]
    fn generation_change_invalidates_enrollment_commit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
        vault.initialize("passphrase").expect("initialize vault");
        let binding = vault.binding_id().unwrap().to_owned();
        let generation = AuthGeneration::new();
        let expected = generation.current();
        assert!(enrollment_is_current(&generation, expected, &vault, &binding));
        generation.invalidate();
        assert!(!enrollment_is_current(&generation, expected, &vault, &binding));
    }
}
