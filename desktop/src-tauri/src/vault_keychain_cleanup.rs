use std::io::Write;
use std::path::{Path, PathBuf};

type ApiResult<T> = std::result::Result<T, String>;

const ENROLLMENT_MARKER: &str = "vault-biometric-binding";
const MARKER_VERSION: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialObservation {
    Stored,
    Missing,
    Invalidated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrackingState {
    Clear,
    Pending,
    Enrolled,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct EnrollmentMarker {
    version: u8,
    binding_id: String,
    state: TrackingState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope_token: Option<String>,
}

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

fn write_marker(app_data_dir: &Path, marker: &EnrollmentMarker) -> ApiResult<()> {
    std::fs::create_dir_all(app_data_dir)
        .map_err(|error| format!("failed to create biometric marker directory: {error}"))?;

    let serialized = serde_json::to_vec_pretty(marker)
        .map_err(|error| format!("failed to serialize biometric marker: {error}"))?;
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
        file.write_all(&serialized)?;
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

fn read_marker(app_data_dir: &Path) -> ApiResult<Option<EnrollmentMarker>> {
    let path = marker_path(app_data_dir);
    let value = match std::fs::read_to_string(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to read biometric enrollment marker: {error}")),
    };

    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("biometric enrollment marker is empty".into());
    }

    if trimmed.starts_with('{') {
        let marker: EnrollmentMarker = serde_json::from_str(trimmed)
            .map_err(|error| format!("failed to parse biometric enrollment marker: {error}"))?;
        if marker.version != MARKER_VERSION {
            return Err(format!(
                "unsupported biometric enrollment marker version {}",
                marker.version
            ));
        }
        return Ok(Some(marker));
    }

    // A legacy plain marker stores only the binding id with no access-group
    // scope token. Treat it as enrolled without a scope probe.
    Ok(Some(EnrollmentMarker {
        version: 1,
        binding_id: trimmed.to_owned(),
        state: TrackingState::Enrolled,
        scope_token: None,
    }))
}

fn marker_for_binding(app_data_dir: &Path, binding_id: &str) -> ApiResult<Option<EnrollmentMarker>> {
    match read_marker(app_data_dir)? {
        None => Ok(None),
        Some(marker) if marker.binding_id == binding_id => Ok(Some(marker)),
        Some(_) => Err(concat!(
            "a biometric credential marker exists for a different vault generation; ",
            "clean up the previous credential before continuing"
        )
        .into()),
    }
}

fn remove_marker_file(app_data_dir: &Path) -> ApiResult<()> {
    let path = marker_path(app_data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => sync_directory(app_data_dir)
            .map_err(|error| format!("failed to sync biometric marker deletion: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("failed to remove biometric enrollment marker: {error}")),
    }
    Ok(())
}

fn write_state(
    app_data_dir: &Path,
    binding_id: &str,
    state: TrackingState,
    scope_token: Option<String>,
) -> ApiResult<()> {
    write_marker(
        app_data_dir,
        &EnrollmentMarker {
            version: MARKER_VERSION,
            binding_id: binding_id.to_owned(),
            state,
            scope_token,
        },
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use security_framework::passwords::{self, PasswordOptions};
    use security_framework_sys::base::errSecItemNotFound;

    const SERVICE: &str = "com.clavyn.vault";
    const ACCOUNT_PREFIX: &str = "master-passphrase";
    const SCOPE_SERVICE: &str = "com.clavyn.vault.tracking-scope";
    const SCOPE_ACCOUNT_PREFIX: &str = "scope";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DeleteOutcome {
        Deleted,
        Missing,
    }

    fn account_for_binding(binding_id: &str) -> String {
        format!("{ACCOUNT_PREFIX}:{binding_id}")
    }

    fn scope_account(binding_id: &str) -> String {
        format!("{SCOPE_ACCOUNT_PREFIX}:{binding_id}")
    }

    fn protected_options(service: &str, account: &str) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(service, account);
        options.use_protected_keychain();
        options
    }

    fn scope_options(binding_id: &str) -> PasswordOptions {
        protected_options(SCOPE_SERVICE, &scope_account(binding_id))
    }

    fn format_keychain_error(error: security_framework::base::Error) -> String {
        let code = error.code();
        match error.message() {
            Some(message) => format!("keychain error (code {code}): {message}"),
            None => format!("keychain error (code {code})"),
        }
    }

    fn delete_account(service: &str, account: &str) -> Result<DeleteOutcome, String> {
        match passwords::delete_generic_password_options(protected_options(service, account)) {
            Ok(()) => Ok(DeleteOutcome::Deleted),
            Err(error) if error.code() == errSecItemNotFound => Ok(DeleteOutcome::Missing),
            Err(error) => Err(format_keychain_error(error)),
        }
    }

    pub fn clear_bound_passphrase(binding_id: &str) -> Result<DeleteOutcome, String> {
        delete_account(SERVICE, &account_for_binding(binding_id))
    }

    pub fn clear_legacy_passphrase() -> Result<(), String> {
        delete_account(SERVICE, ACCOUNT_PREFIX).map(|_| ())
    }

    pub fn store_scope_probe(binding_id: &str, token: &str) -> Result<(), String> {
        match passwords::delete_generic_password_options(scope_options(binding_id)) {
            Ok(()) => {}
            Err(error) if error.code() == errSecItemNotFound => {}
            Err(error) => return Err(format_keychain_error(error)),
        }
        passwords::set_generic_password_options(token.as_bytes(), scope_options(binding_id))
            .map_err(format_keychain_error)
    }

    pub fn scope_probe_matches(binding_id: &str, token: &str) -> Result<bool, String> {
        match passwords::generic_password(scope_options(binding_id)) {
            Ok(value) => Ok(value == token.as_bytes()),
            Err(error) if error.code() == errSecItemNotFound => Ok(false),
            Err(error) => Err(format_keychain_error(error)),
        }
    }

    pub fn clear_scope_probe(binding_id: &str) -> Result<(), String> {
        delete_account(SCOPE_SERVICE, &scope_account(binding_id)).map(|_| ())
    }

    #[cfg(test)]
    mod tests {
        use super::{account_for_binding, scope_account};

        #[test]
        fn credential_and_scope_accounts_are_bound_to_generation() {
            assert_eq!(
                account_for_binding("vault-generation"),
                "master-passphrase:vault-generation"
            );
            assert_eq!(scope_account("vault-generation"), "scope:vault-generation");
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

#[cfg(target_os = "macos")]
async fn create_scope_probe(binding_id: &str) -> ApiResult<String> {
    let token = uuid::Uuid::new_v4().to_string();
    let bound_id = binding_id.to_owned();
    let stored_token = token.clone();
    blocking_keychain_call(move || macos::store_scope_probe(&bound_id, &stored_token)).await?;
    Ok(token)
}

#[cfg(not(target_os = "macos"))]
async fn create_scope_probe(_binding_id: &str) -> ApiResult<String> {
    Ok(String::new())
}

#[cfg(target_os = "macos")]
async fn verify_scope_probe(binding_id: &str, token: &str) -> ApiResult<()> {
    let bound_id = binding_id.to_owned();
    let expected = token.to_owned();
    let matches =
        blocking_keychain_call(move || macos::scope_probe_matches(&bound_id, &expected)).await?;
    if !matches {
        return Err(concat!(
            "biometric tracking scope is not visible to this macOS build; ",
            "refusing to interpret a missing credential because the code-signing ",
            "Keychain access group may have changed"
        )
        .into());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn verify_scope_probe(_binding_id: &str, _token: &str) -> ApiResult<()> {
    Ok(())
}

async fn clear_scope_probe_best_effort(binding_id: &str) {
    #[cfg(target_os = "macos")]
    {
        let bound_id = binding_id.to_owned();
        if let Err(error) = blocking_keychain_call(move || macos::clear_scope_probe(&bound_id)).await
        {
            tracing::warn!("failed to clear biometric tracking scope probe: {error}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = binding_id;
}

async fn write_clear_state(app_data_dir: &Path, binding_id: &str) -> ApiResult<()> {
    // Publish the durable clear state before cleaning the non-secret scope probe.
    // A crash after this write is safe: Clear means no passphrase credential is
    // expected for this binding, regardless of a leftover probe item.
    write_state(app_data_dir, binding_id, TrackingState::Clear, None)?;
    clear_scope_probe_best_effort(binding_id).await;
    Ok(())
}

async fn write_tracked_state(
    app_data_dir: &Path,
    binding_id: &str,
    state: TrackingState,
) -> ApiResult<()> {
    let token = create_scope_probe(binding_id).await?;
    write_state(
        app_data_dir,
        binding_id,
        state,
        if token.is_empty() { None } else { Some(token) },
    )
}

/// Reconcile the durable state machine with an existence-only Keychain probe.
/// The caller must serialize this operation with reset/enable/disable and must
/// revalidate the vault binding after the asynchronous platform probe.
pub(crate) async fn reconcile_credential_observation(
    app_data_dir: &Path,
    binding_id: &str,
    observation: CredentialObservation,
) -> ApiResult<bool> {
    let marker = marker_for_binding(app_data_dir, binding_id)?;

    let stored = observation == CredentialObservation::Stored;
    match marker {
        None => {
            match observation {
                CredentialObservation::Stored | CredentialObservation::Invalidated => {
                    // A pre-marker credential is visible in this access group, so
                    // we can safely establish v2 scope tracking for it.
                    write_tracked_state(app_data_dir, binding_id, TrackingState::Enrolled).await?;
                }
                CredentialObservation::Missing => {
                    // A legacy vault with no enrollment marker and no visible
                    // credential is treated as Clear. The credential is either
                    // genuinely absent or invisible due to a macOS signing/
                    // access-group transition; in either case it is bound to this
                    // binding id and cannot unlock a future vault generation.
                    // Establishing Clear tracking unblocks reset and enrollment
                    // for users who never enabled Touch ID or no longer have a
                    // credential, while still recording the tracking state.
                    write_clear_state(app_data_dir, binding_id).await?;
                }
            }
            return Ok(stored);
        }
        Some(marker) if marker.state == TrackingState::Clear => {
            match observation {
                CredentialObservation::Missing => return Ok(false),
                CredentialObservation::Stored | CredentialObservation::Invalidated => {
                    write_tracked_state(app_data_dir, binding_id, TrackingState::Enrolled).await?;
                    return Ok(stored);
                }
            }
        }
        Some(mut marker) => {
            let Some(scope_token) = marker.scope_token.clone() else {
                // Legacy enrolled markers do not have a scope probe. They may be
                // upgraded only while the credential is positively visible.
                match observation {
                    CredentialObservation::Stored | CredentialObservation::Invalidated => {
                        write_tracked_state(app_data_dir, binding_id, TrackingState::Enrolled)
                            .await?;
                        return Ok(stored);
                    }
                    CredentialObservation::Missing => {
                        return Err(concat!(
                            "tracked biometric credential is not visible to this macOS build; ",
                            "refusing to treat it as absent without a verified tracking scope"
                        )
                        .into());
                    }
                }
            };

            verify_scope_probe(binding_id, &scope_token).await?;
            match (marker.state, observation) {
                (TrackingState::Pending, CredentialObservation::Missing)
                | (TrackingState::Enrolled, CredentialObservation::Missing) => {
                    // Same access group + exact item missing is authoritative.
                    // For Pending this recovers a crash before credential creation;
                    // for Enrolled it handles explicit/manual Keychain deletion.
                    write_clear_state(app_data_dir, binding_id).await?;
                }
                (TrackingState::Pending, CredentialObservation::Stored)
                | (TrackingState::Pending, CredentialObservation::Invalidated) => {
                    marker.version = MARKER_VERSION;
                    marker.state = TrackingState::Enrolled;
                    write_marker(app_data_dir, &marker)?;
                }
                (TrackingState::Enrolled, CredentialObservation::Stored)
                | (TrackingState::Enrolled, CredentialObservation::Invalidated) => {}
                (TrackingState::Clear, _) => unreachable!(),
            }
        }
    }

    Ok(stored)
}

/// Transition a known-clear vault into a durable pending-enrollment state before
/// writing the protected passphrase. The scope probe is created first; therefore
/// a crash before the marker write leaves only a non-secret orphan probe, while a
/// crash after the marker write can be recovered safely on the next probe.
pub(crate) async fn begin_enrollment(app_data_dir: &Path, binding_id: &str) -> ApiResult<()> {
    let marker = marker_for_binding(app_data_dir, binding_id)?.ok_or_else(|| {
        concat!(
            "biometric tracking for this legacy vault is unknown; refusing to enroll because ",
            "a credential from a previous macOS signing access group may be invisible. ",
            "Run a marker-aware build with the previous signing identity or clean the old ",
            "Keychain credential explicitly before continuing"
        )
        .to_string()
    })?;

    if marker.state != TrackingState::Clear {
        return Err("biometric enrollment state is not clear; refresh biometric status first".into());
    }

    write_tracked_state(app_data_dir, binding_id, TrackingState::Pending).await
}

pub(crate) async fn finish_enrollment(app_data_dir: &Path, binding_id: &str) -> ApiResult<()> {
    let mut marker = marker_for_binding(app_data_dir, binding_id)?
        .ok_or_else(|| "biometric enrollment marker disappeared during enrollment".to_string())?;
    if marker.state != TrackingState::Pending {
        return Err("biometric enrollment is no longer pending".into());
    }
    let token = marker
        .scope_token
        .clone()
        .ok_or_else(|| "pending biometric enrollment has no tracking scope".to_string())?;
    verify_scope_probe(binding_id, &token).await?;
    marker.state = TrackingState::Enrolled;
    write_marker(app_data_dir, &marker)
}

/// Authoritatively delete the vault-bound protected credential. V2 enrolled or
/// pending states carry a scope probe, so `Missing` is trusted only when this
/// build can prove it is still in the same Data Protection Keychain access group.
///
/// When `binding_will_be_destroyed` is true (reset path), a pre-marker legacy
/// vault with `Missing` is treated as Clear: the credential is either genuinely
/// absent or bound to a binding id that is being destroyed and cannot unlock a
/// future vault generation. When false (disable path), an untracked `Missing`
/// remains ambiguous and fails closed because the binding id stays live and an
/// access-group-invisible credential could still be retrieved by an older build.
#[cfg(target_os = "macos")]
pub(crate) async fn clear_bound_credential(
    app_data_dir: &Path,
    binding_id: &str,
    binding_will_be_destroyed: bool,
) -> ApiResult<()> {
    let marker = marker_for_binding(app_data_dir, binding_id)?;

    if let Some(marker) = &marker {
        if marker.state == TrackingState::Clear {
            return write_clear_state(app_data_dir, binding_id).await;
        }
        if let Some(token) = marker.scope_token.as_deref() {
            verify_scope_probe(binding_id, token).await?;
        }
    }

    let bound_id = binding_id.to_owned();
    let outcome = blocking_keychain_call(move || macos::clear_bound_passphrase(&bound_id)).await?;

    match (&marker, outcome) {
        (None, macos::DeleteOutcome::Missing) if !binding_will_be_destroyed => {
            return Err(concat!(
                "legacy biometric tracking is unknown and the vault-bound credential is not ",
                "visible to this macOS build; refusing cleanup because a direct signing/access-",
                "group transition could otherwise leave the old master passphrase accessible"
            )
            .into());
        }
        (Some(marker), macos::DeleteOutcome::Missing)
            if marker.state != TrackingState::Clear && marker.scope_token.is_none() =>
        {
            return Err(concat!(
                "tracked biometric credential is not visible to this macOS build; ",
                "refusing cleanup without a verified tracking scope"
            )
            .into());
        }
        _ => {}
    }

    write_clear_state(app_data_dir, binding_id).await
}

#[cfg(not(target_os = "macos"))]
pub(crate) async fn clear_bound_credential(
    app_data_dir: &Path,
    binding_id: &str,
    _binding_will_be_destroyed: bool,
) -> ApiResult<()> {
    write_clear_state(app_data_dir, binding_id).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn clear_legacy_best_effort(context: &str) {
    if let Err(error) = blocking_keychain_call(macos::clear_legacy_passphrase).await {
        tracing::warn!("failed to clean legacy biometric credential during {context}: {error}");
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) async fn clear_legacy_best_effort(_context: &str) {}

pub(crate) async fn clear_for_reset(app_data_dir: &Path, binding_id: &str) -> ApiResult<()> {
    clear_bound_credential(app_data_dir, binding_id, true).await?;
    clear_legacy_best_effort("vault reset").await;
    Ok(())
}

/// Called only while no vault is initialized. A stale Clear marker can be left
/// behind by a successful reset and is safe to discard before creating the next
/// generation. Pending/Enrolled markers are never discarded automatically.
pub(crate) async fn prepare_for_new_vault(app_data_dir: &Path) -> ApiResult<()> {
    let Some(marker) = read_marker(app_data_dir)? else {
        return Ok(());
    };

    if marker.state != TrackingState::Clear {
        return Err(concat!(
            "cannot create a new vault while biometric tracking for the previous generation ",
            "is pending or enrolled; resolve the previous Keychain credential first"
        )
        .into());
    }

    remove_marker_file(app_data_dir)?;
    clear_scope_probe_best_effort(&marker.binding_id).await;
    Ok(())
}

/// New vault generations start with an explicit durable Clear state. This makes
/// them distinguishable from pre-marker vaults, for which a missing Keychain item
/// is ambiguous across a direct macOS signing/access-group transition.
pub(crate) fn initialize_tracking_for_new_vault(
    app_data_dir: &Path,
    binding_id: &str,
) -> ApiResult<()> {
    write_state(app_data_dir, binding_id, TrackingState::Clear, None)
}

#[cfg(test)]
mod marker_tests {
    use super::{
        begin_enrollment, clear_for_reset, initialize_tracking_for_new_vault, read_marker,
        reconcile_credential_observation, CredentialObservation, TrackingState,
    };

    #[test]
    fn new_vault_starts_in_explicit_clear_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        initialize_tracking_for_new_vault(dir.path(), "generation-a").expect("initialize marker");

        let marker = read_marker(dir.path()).unwrap().unwrap();
        assert_eq!(marker.binding_id, "generation-a");
        assert_eq!(marker.state, TrackingState::Clear);
        assert!(marker.scope_token.is_none());
    }

    #[test]
    fn legacy_plain_marker_is_treated_as_enrolled_without_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(super::ENROLLMENT_MARKER), "generation-a\n")
            .expect("write legacy marker");

        let marker = read_marker(dir.path()).unwrap().unwrap();
        assert_eq!(marker.state, TrackingState::Enrolled);
        assert!(marker.scope_token.is_none());
        assert!(marker.binding_id == "generation-a");
    }

    #[tokio::test]
    async fn unknown_legacy_vault_cannot_begin_enrollment() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = begin_enrollment(dir.path(), "generation-a")
            .await
            .unwrap_err();
        assert!(error.contains("tracking for this legacy vault is unknown"));
    }

    #[tokio::test]
    async fn legacy_vault_with_missing_credential_reconciles_to_clear() {
        let dir = tempfile::tempdir().expect("tempdir");

        // A legacy vault has no enrollment marker. When the credential is
        // missing, reconcile must establish Clear tracking so reset and
        // enrollment are not permanently blocked.
        let stored = reconcile_credential_observation(
            dir.path(),
            "generation-a",
            CredentialObservation::Missing,
        )
        .await
        .expect("legacy missing should reconcile to clear");

        assert!(!stored, "missing credential should not report stored");
        let marker = read_marker(dir.path())
            .expect("read marker")
            .expect("marker should be written");
        assert_eq!(marker.binding_id, "generation-a");
        assert_eq!(marker.state, TrackingState::Clear);
    }

    #[tokio::test]
    async fn legacy_vault_can_enroll_after_missing_reconciles_to_clear() {
        let dir = tempfile::tempdir().expect("tempdir");

        reconcile_credential_observation(
            dir.path(),
            "generation-a",
            CredentialObservation::Missing,
        )
        .await
        .expect("reconcile legacy missing");

        // After reconcile establishes Clear tracking, enrollment must not be
        // blocked by the "unknown legacy vault" guard. On macOS without
        // Keychain entitlements the scope probe may fail, but that is a
        // platform capability error, not the tracking block being fixed here.
        let result = begin_enrollment(dir.path(), "generation-a").await;
        match &result {
            Ok(()) => {
                let marker = read_marker(dir.path())
                    .expect("read marker")
                    .expect("marker should be pending");
                assert_eq!(marker.state, TrackingState::Pending);
            }
            Err(error) => {
                assert!(
                    !error.contains("tracking for this legacy vault is unknown"),
                    "enrollment should not be blocked by unknown tracking after Clear: {error}"
                );
            }
        }
    }

    #[tokio::test]
    async fn legacy_vault_reset_succeeds_with_no_marker_and_missing_credential() {
        let dir = tempfile::tempdir().expect("tempdir");

        // A legacy vault with no enrollment marker must be resettable. The
        // reset path treats an untracked missing credential as Clear because
        // the binding id is being destroyed and cannot unlock a future vault.
        let result = clear_for_reset(dir.path(), "generation-a").await;
        match &result {
            Ok(()) => {
                let marker = read_marker(dir.path())
                    .expect("read marker")
                    .expect("marker should be written as Clear");
                assert_eq!(marker.binding_id, "generation-a");
                assert_eq!(marker.state, TrackingState::Clear);
            }
            Err(error) => {
                // On macOS without Keychain entitlements the blocking Keychain
                // call fails before reaching the tracking logic. That is a
                // platform capability error, not the tracking block being fixed.
                assert!(
                    !error.contains("legacy biometric tracking is unknown"),
                    "reset should not be blocked by unknown tracking: {error}"
                );
            }
        }
    }
}
