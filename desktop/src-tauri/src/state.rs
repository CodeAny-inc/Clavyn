use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::session::SessionManager;
use clavyn_core::sftp::SftpManager;
use clavyn_core::store::Store;
use clavyn_core::vault::Vault;
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

/// Shared app state. The vault passphrase is held in memory only for the
/// duration of an unlocked session; it is never persisted. On lock, it is
/// zeroized.
pub struct AppState {
    pub store: Mutex<Store>,
    pub vault: Mutex<Vault>,
    pub known_hosts: Arc<Mutex<KnownHosts>>,
    pub passphrase: Mutex<Option<zeroize::Zeroizing<String>>>,
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
    /// Kept alive so the shell is not reaped, and polled with `try_wait` to
    /// tell a live terminal from one whose shell has already exited.
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl AppState {
    /// Load persisted state. A file that exists but cannot be parsed is an
    /// error: starting with empty state would discard the user's hosts and, for
    /// known_hosts, silently drop every pinned server key.
    pub fn init(app: &AppHandle, app_data: PathBuf) -> Result<Arc<Self>> {
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
            passphrase: Mutex::new(None),
            auth_generation: AuthGeneration::new(),
            biometric_mutation: Mutex::new(()),
            sessions,
            sftp,
            local_terminals: Mutex::new(std::collections::HashMap::new()),
            app_data_dir: app_data,
        }))
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
    use super::AuthGeneration;

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
}
