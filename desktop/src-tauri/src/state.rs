use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::session::SessionManager;
use clavyn_core::sftp::SftpManager;
use clavyn_core::store::Store;
use clavyn_core::vault::Vault;
use clavyn_core::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    pub local_terminals: Mutex<LocalTerminals>,
    pub app_data_dir: PathBuf,
}

/// Live local terminals and the ids currently being opened.
///
/// Opening a terminal has to release the lock to spawn a PTY and a shell, which
/// is slow and blocking. Checking the cap and then spawning would let every
/// request in a burst pass the check before any of them inserted, so the shells
/// would already exist by the time the surplus was refused. A reservation is
/// taken under the same lock as the check instead, so the id and the slot are
/// claimed before anything is spawned and the cap bounds processes rather than
/// map entries.
#[derive(Default)]
pub struct LocalTerminals {
    live: std::collections::HashMap<String, LocalTerminal>,
    opening: std::collections::HashSet<String>,
}

impl LocalTerminals {
    /// Slots in use, counting terminals still being opened.
    pub fn len(&self) -> usize {
        self.live.len() + self.opening.len()
    }

    /// Whether an id is taken, whether or not its shell exists yet.
    pub fn contains(&self, session_id: &str) -> bool {
        self.live.contains_key(session_id) || self.opening.contains(session_id)
    }

    /// Ids of terminals that are ready to use. A reserved id is deliberately
    /// absent: nothing can be written to it yet.
    pub fn live_ids(&self) -> impl Iterator<Item = &String> {
        self.live.keys()
    }

    pub fn get_live(&self, session_id: &str) -> Option<&LocalTerminal> {
        self.live.get(session_id)
    }

    pub fn get_live_mut(&mut self, session_id: &str) -> Option<&mut LocalTerminal> {
        self.live.get_mut(session_id)
    }

    pub fn reserve(&mut self, session_id: String) {
        self.opening.insert(session_id);
    }

    /// Turns a reservation into a usable terminal. Returns the terminal back if
    /// the reservation is gone, which means the session was closed while its
    /// shell was starting and the caller has to tear it down.
    pub fn fulfil(
        &mut self,
        session_id: &str,
        terminal: LocalTerminal,
    ) -> std::result::Result<(), LocalTerminal> {
        if !self.opening.remove(session_id) {
            return Err(terminal);
        }
        self.live.insert(session_id.to_string(), terminal);
        Ok(())
    }

    /// Drops a reservation whose shell never started.
    pub fn release(&mut self, session_id: &str) {
        self.opening.remove(session_id);
    }

    /// Removes a session, live or still opening. The removed terminal gives up
    /// its session id here rather than when it is dropped, so the id is free of
    /// its old reader before the lock this runs under is released.
    pub fn remove(&mut self, session_id: &str) -> Option<LocalTerminal> {
        self.opening.remove(session_id);
        let removed = self.live.remove(session_id);
        if let Some(terminal) = &removed {
            terminal.disown();
        }
        removed
    }

    /// Takes out the terminals `exited` reports as finished, returning them so
    /// the caller can drop them with the lock released. A reservation has no
    /// child to poll, so it is never reaped.
    pub fn reap_exited(
        &mut self,
        exited: impl Fn(&mut LocalTerminal) -> bool,
    ) -> Vec<LocalTerminal> {
        let finished: Vec<String> = self
            .live
            .iter_mut()
            .filter_map(|(id, term)| exited(term).then(|| id.clone()))
            .collect();
        finished
            .iter()
            .filter_map(|id| self.live.remove(id))
            .inspect(|terminal| terminal.disown())
            .collect()
    }
}

/// A local terminal session backed by portable-pty.
pub struct LocalTerminal {
    pub writer: Box<dyn std::io::Write + Send>,
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    /// Kept alive so the shell is not reaped, and polled with `try_wait` to
    /// tell a live terminal from one whose shell has already exited.
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Cleared when this terminal stops holding its session id.
    ///
    /// The thread draining the PTY outlives the map entry: it is detached, and
    /// a shell that has exited is only noticed when the entry is reaped, by
    /// which time the reader may still have buffered output to deliver and a
    /// closing notice to send. Session ids are reusable, and both events carry
    /// nothing but the id, so a replacement terminal on the same id would
    /// receive the old shell's output and then be disconnected by its
    /// `session-closed`. The reader checks this before it emits.
    pub owns_session_id: Arc<AtomicBool>,
}

impl LocalTerminal {
    /// Marks the terminal as no longer holding its session id.
    pub fn disown(&self) {
        self.owns_session_id.store(false, Ordering::Release);
    }
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
            local_terminals: Mutex::new(LocalTerminals::default()),
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
mod local_terminal_ownership_tests {
    use super::*;
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};

    /// A real terminal, plus the flag its reader thread would consult. The PTY
    /// handles are what the map stores, so there is no way to exercise removal
    /// without them.
    fn terminal() -> (LocalTerminal, Arc<AtomicBool>) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pty can be opened");
        let shell = if cfg!(target_os = "windows") {
            "cmd.exe"
        } else {
            "/bin/sh"
        };
        let child = pair
            .slave
            .spawn_command(CommandBuilder::new(shell))
            .expect("a shell can be spawned");
        let writer = pair.master.take_writer().expect("the master has a writer");
        let owns_session_id = Arc::new(AtomicBool::new(true));
        (
            LocalTerminal {
                writer,
                master: pair.master,
                child,
                owns_session_id: owns_session_id.clone(),
            },
            owns_session_id,
        )
    }

    fn live(locals: &mut LocalTerminals, session_id: &str) -> Arc<AtomicBool> {
        let (terminal, owns) = terminal();
        locals.reserve(session_id.to_string());
        assert!(locals.fulfil(session_id, terminal).is_ok());
        owns
    }

    #[test]
    fn closing_a_terminal_hands_its_session_id_back() {
        let mut locals = LocalTerminals::default();
        let owns = live(&mut locals, "local-0");
        assert!(owns.load(Ordering::Acquire));

        // The id is free for reuse from here, so the reader still draining the
        // old shell must stop addressing it.
        let mut closed = locals.remove("local-0").expect("the terminal was live");
        assert!(!owns.load(Ordering::Acquire));
        let _ = closed.child.kill();
    }

    #[test]
    fn reaping_an_exited_terminal_hands_its_session_id_back() {
        let mut locals = LocalTerminals::default();
        let owns = live(&mut locals, "local-0");

        let reaped = locals.reap_exited(|_| true);
        assert_eq!(reaped.len(), 1);
        assert!(!owns.load(Ordering::Acquire));
        for mut terminal in reaped {
            let _ = terminal.child.kill();
        }
    }

    #[test]
    fn a_terminal_that_stays_open_keeps_its_session_id() {
        let mut locals = LocalTerminals::default();
        let owns = live(&mut locals, "local-0");

        assert!(locals.reap_exited(|_| false).is_empty());
        assert!(locals.remove("other").is_none());
        assert!(owns.load(Ordering::Acquire));

        let mut open = locals.remove("local-0").expect("the terminal was live");
        let _ = open.child.kill();
    }
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
