use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::session::SessionManager;
use clavyn_core::sftp::SftpManager;
use clavyn_core::store::Store;
use clavyn_core::vault::Vault;
use clavyn_core::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

/// Sink a session's terminal output is delivered to.
///
/// Bytes travel as a raw IPC payload rather than a serialized event: a JSON
/// array of decimal numbers costs roughly three times the wire size and is
/// parsed as JavaScript source, which dominates the cost of bulk output. One
/// sink per session also keeps a pane from receiving every other session's
/// bytes only to discard them.
pub type OutputSink = Channel<InvokeResponseBody>;

/// Frame that marks the end of a session's output stream.
///
/// A batch is never empty, so a zero-length payload is unambiguous. It travels
/// on the session's own sink, which stamps every frame with an index and holds
/// a frame back until its predecessors have been delivered; that is what keeps
/// the end of the stream behind the last batch even when the batch is large
/// enough to take the transport's asynchronous route. The `session-closed`
/// event carries no index and cannot provide that ordering on its own.
pub fn end_of_output() -> InvokeResponseBody {
    InvokeResponseBody::Raw(Vec::new())
}

/// Registry of live output sinks, keyed by session id.
///
/// Guarded by a blocking mutex because the core data callback is synchronous.
/// It is only ever held long enough to look a sink up, never across delivery.
pub type OutputSinks = Arc<std::sync::Mutex<HashMap<String, OutputSink>>>;

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
    pub output_sinks: OutputSinks,
    pub local_terminals: Mutex<HashMap<String, LocalTerminal>>,
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
    pub fn init(app: &AppHandle, app_data: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&app_data).ok();
        let store = Store::load(app_data.join("store.json"))?;
        let vault = Vault::open(app_data.join("vault.json"))?;
        let known_hosts = KnownHosts::load(app_data.join("known_hosts.json"))?;

        let output_sinks: OutputSinks = Arc::new(std::sync::Mutex::new(HashMap::new()));

        let sinks = output_sinks.clone();
        let data_callback = Arc::new(move |sid: &str, data: &[u8]| {
            let sink = sinks.lock().ok().and_then(|map| map.get(sid).cloned());
            if let Some(sink) = sink {
                let _ = sink.send(InvokeResponseBody::Raw(data.to_vec()));
            }
        });

        let sinks = output_sinks.clone();
        let app_handle = app.clone();
        let close_callback = Arc::new(move |sid: &str, reason: &str| {
            // End the stream on the sink itself before announcing the close, so
            // the frontend can tell a batch that is still in flight from one
            // that will never arrive. The event that follows says why the
            // session ended; the frame says that nothing more is coming.
            let sink = sinks.lock().ok().and_then(|mut map| map.remove(sid));
            if let Some(sink) = sink {
                let _ = sink.send(end_of_output());
            }
            let _ = app_handle.emit(
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
            output_sinks,
            local_terminals: Mutex::new(HashMap::new()),
            app_data_dir: app_data,
        }))
    }

    /// Routes a session's output to `sink` until the session ends.
    pub fn register_output(&self, session_id: String, sink: OutputSink) {
        if let Ok(mut map) = self.output_sinks.lock() {
            map.insert(session_id, sink);
        }
    }

    /// Stops routing output for a session. Safe to call for a session that was
    /// never registered.
    pub fn release_output(&self, session_id: &str) {
        if let Ok(mut map) = self.output_sinks.lock() {
            map.remove(session_id);
        }
    }
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
