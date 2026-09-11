use crate::host_key_prompt::PromptGate as HostKeyPromptGate;
use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::session::SessionManager;
use clavyn_core::sftp::SftpManager;
use clavyn_core::store::Store;
use clavyn_core::vault::{Vault, VaultKey};
use clavyn_core::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

/// Monotonic generation used to invalidate unlock attempts that started before
/// a newer lock (or vault initialization) operation. The passphrase mutex still
/// serializes the final state write; this generation additionally prevents a
/// long-running biometric request from resurrecting an older unlocked state.
pub struct AuthGeneration {
    value: AtomicU64,
}

impl AuthGeneration {
    pub fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }

    pub fn current(&self) -> u64 {
        self.value.load(Ordering::SeqCst)
    }

    pub fn invalidate(&self) -> u64 {
        self.value
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1)
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.current() == generation
    }
}

/// Credentials of an unlocked vault: the passphrase the user supplied and the
/// Argon2id master key derived from it, tagged with the vault generation that
/// produced them.
///
/// Both are zeroized on drop and both live in this one value, so clearing the
/// vault's authentication state cannot clear one and leave the other resident.
/// Caching the key is what keeps per-operation cost off the KDF; the passphrase
/// was already resident for the same window, so nothing new outlives a lock.
pub struct VaultSession {
    passphrase: zeroize::Zeroizing<String>,
    key: VaultKey,
    binding_id: String,
}

impl VaultSession {
    pub fn new(
        passphrase: zeroize::Zeroizing<String>,
        key: VaultKey,
        binding_id: String,
    ) -> Self {
        Self {
            passphrase,
            key,
            binding_id,
        }
    }

    pub fn passphrase(&self) -> zeroize::Zeroizing<String> {
        self.passphrase.clone()
    }

    /// The cached master key, but only for the vault generation it was derived
    /// from. A vault that has been destroyed and re-created carries a new
    /// binding id, so a key that no longer opens anything is never handed back
    /// even if some path forgets to clear this session.
    pub fn key_for(&self, binding_id: &str) -> Option<VaultKey> {
        (self.binding_id == binding_id).then(|| self.key.clone())
    }
}

/// The single home for unlocked-vault credentials.
///
/// Every path that ends an unlocked session — an explicit lock, a destructive
/// reset, an unlock superseded by a newer generation — goes through `clear` or
/// `unlock_if_current`, so there is one place to audit.
pub struct VaultSessionSlot(Mutex<Option<VaultSession>>);

impl VaultSessionSlot {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub async fn is_unlocked(&self) -> bool {
        self.0.lock().await.is_some()
    }

    pub async fn passphrase(&self) -> Option<zeroize::Zeroizing<String>> {
        self.0.lock().await.as_ref().map(VaultSession::passphrase)
    }

    pub async fn key_for(&self, binding_id: &str) -> Option<VaultKey> {
        self.0
            .lock()
            .await
            .as_ref()
            .and_then(|session| session.key_for(binding_id))
    }

    /// Install credentials only if no lock, reset, or initialization has
    /// advanced the auth generation since the caller started. Returns whether
    /// the session was installed.
    pub async fn unlock_if_current(
        &self,
        auth_generation: &AuthGeneration,
        generation: u64,
        session: VaultSession,
    ) -> bool {
        let mut slot = self.0.lock().await;
        if !auth_generation.is_current(generation) {
            return false;
        }
        *slot = Some(session);
        true
    }

    /// Install credentials for a freshly created vault. Publishing them also
    /// advances the auth generation, so an unlock still in flight against the
    /// previous vault can never commit on top of the new one.
    pub async fn unlock_new_vault_if_current(
        &self,
        auth_generation: &AuthGeneration,
        generation: u64,
        session: VaultSession,
    ) -> bool {
        let mut slot = self.0.lock().await;
        if !auth_generation.is_current(generation) {
            return false;
        }
        auth_generation.invalidate();
        *slot = Some(session);
        true
    }

    pub async fn clear(&self) {
        *self.0.lock().await = None;
    }
}

/// Shared app state. The vault passphrase and its derived master key are held
/// in memory only for the duration of an unlocked session; they are never
/// persisted. On lock, both are zeroized.
pub struct AppState {
    pub store: Mutex<Store>,
    pub vault: Mutex<Vault>,
    pub known_hosts: Arc<Mutex<KnownHosts>>,
    /// Serializes the native confirmation shown before a host key is forgotten
    /// or replaced, so a caller cannot stack dialogs on top of each other.
    pub host_key_prompt: HostKeyPromptGate,
    pub vault_session: VaultSessionSlot,
    pub auth_generation: AuthGeneration,
    /// Serializes Keychain credential creation/deletion with destructive vault
    /// reset. This ensures reset can authoritatively remove the old vault-bound
    /// credential before destroying the binding id needed to address it.
    pub biometric_mutation: Mutex<()>,
    pub sessions: Arc<SessionManager>,
    pub sftp: Arc<SftpManager>,
    pub local_terminals: Mutex<std::collections::HashMap<String, LocalTerminal>>,
    pub app_data_dir: PathBuf,
}

/// A local terminal session backed by portable-pty.
pub struct LocalTerminal {
    pub writer: Box<dyn std::io::Write + Send>,
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub _child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl AppState {
    /// Load persisted state. A file that exists but cannot be parsed is an
    /// error: starting with empty state would discard the user's hosts and, for
    /// known_hosts, silently drop every pinned server key.
    pub fn init<R: tauri::Runtime>(app: &AppHandle<R>, app_data: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&app_data).ok();
        let store = Store::load(app_data.join("store.json"))?;
        let vault = Vault::open(app_data.join("vault.json"))?;
        let known_hosts = KnownHosts::load(app_data.join("known_hosts.json"))?;

        let app_handle = app.clone();
        let data_callback = Arc::new(move |sid: &str, data: &[u8]| {
            let _ = app_handle.emit(
                "session-data",
                SessionDataEvent {
                    session_id: sid.to_string(),
                    data: data.to_vec(),
                },
            );
        });

        let app_handle2 = app.clone();
        let close_callback = Arc::new(move |sid: &str, reason: &str| {
            let _ = app_handle2.emit(
                "session-closed",
                SessionClosedEvent {
                    session_id: sid.to_string(),
                    reason: reason.to_string(),
                },
            );
        });

        let sessions = Arc::new(SessionManager::new(data_callback, close_callback));
        let sftp = Arc::new(SftpManager::new());

        Ok(Arc::new(Self {
            store: Mutex::new(store),
            vault: Mutex::new(vault),
            known_hosts: Arc::new(Mutex::new(known_hosts)),
            host_key_prompt: HostKeyPromptGate::new(),
            vault_session: VaultSessionSlot::new(),
            auth_generation: AuthGeneration::new(),
            biometric_mutation: Mutex::new(()),
            sessions,
            sftp,
            local_terminals: Mutex::new(std::collections::HashMap::new()),
            app_data_dir: app_data,
        }))
    }

    /// A held guard on the vault together with the master key for its current
    /// generation, or an error when the vault is locked or not yet created.
    ///
    /// The vault mutex is taken before the session slot, and the slot is never
    /// held across a vault acquisition, so the two locks cannot form a cycle.
    pub async fn unlocked_vault(
        &self,
    ) -> std::result::Result<(VaultKey, tokio::sync::MutexGuard<'_, Vault>), String> {
        let vault = self.vault.lock().await;
        let binding_id = vault
            .binding_id()
            .ok_or_else(|| "vault is not initialized".to_string())?
            .to_owned();
        let key = self
            .vault_session
            .key_for(&binding_id)
            .await
            .ok_or_else(|| "vault is locked".to_string())?;
        Ok((key, vault))
    }
}

#[derive(Clone, serde::Serialize)]
pub struct SessionDataEvent {
    pub session_id: String,
    pub data: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
pub struct SessionClosedEvent {
    pub session_id: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::{AuthGeneration, VaultSession, VaultSessionSlot};

    fn session(binding_id: &str) -> VaultSession {
        VaultSession::new(
            zeroize::Zeroizing::new("correct horse battery staple".to_string()),
            zeroize::Zeroizing::new([7u8; 32]),
            binding_id.to_string(),
        )
    }

    #[test]
    fn invalidating_auth_generation_rejects_older_attempts() {
        let generation = AuthGeneration::new();
        let original = generation.current();
        assert!(generation.is_current(original));

        let next = generation.invalidate();
        assert_ne!(original, next);
        assert!(!generation.is_current(original));
        assert!(generation.is_current(next));
    }

    #[tokio::test]
    async fn locking_clears_the_passphrase_and_the_derived_key_together() {
        let slot = VaultSessionSlot::new();
        slot.unlock_if_current(&AuthGeneration::new(), 0, session("binding-a"))
            .await;
        assert!(slot.is_unlocked().await);
        assert!(slot.passphrase().await.is_some());
        assert!(slot.key_for("binding-a").await.is_some());

        slot.clear().await;

        assert!(!slot.is_unlocked().await);
        assert!(slot.passphrase().await.is_none());
        assert!(slot.key_for("binding-a").await.is_none());
    }

    #[tokio::test]
    async fn a_key_from_an_earlier_vault_generation_is_never_returned() {
        let slot = VaultSessionSlot::new();
        slot.unlock_if_current(&AuthGeneration::new(), 0, session("binding-before-reset"))
            .await;

        // A reset followed by a fresh initialization produces a new binding id.
        assert!(slot.key_for("binding-after-reset").await.is_none());
        assert!(slot.key_for("binding-before-reset").await.is_some());
    }

    #[tokio::test]
    async fn an_unlock_superseded_by_a_newer_generation_installs_nothing() {
        let slot = VaultSessionSlot::new();
        let generation = AuthGeneration::new();
        let started = generation.current();

        // A lock (or reset, or initialization) lands while the unlock is in
        // flight; the older attempt must not resurrect an unlocked vault.
        generation.invalidate();

        assert!(
            !slot
                .unlock_if_current(&generation, started, session("binding-a"))
                .await
        );
        assert!(!slot.is_unlocked().await);
        assert!(slot.key_for("binding-a").await.is_none());
    }

    #[tokio::test]
    async fn a_current_unlock_replaces_the_previous_session_wholesale() {
        let slot = VaultSessionSlot::new();
        let generation = AuthGeneration::new();
        slot.unlock_if_current(&generation, generation.current(), session("binding-a"))
            .await;

        let next = generation.invalidate();
        assert!(
            slot.unlock_if_current(&generation, next, session("binding-b"))
                .await
        );
        assert!(slot.key_for("binding-a").await.is_none());
        assert!(slot.key_for("binding-b").await.is_some());
    }
}
