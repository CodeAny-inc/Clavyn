use crate::host_key_prompt::PromptGate as HostKeyPromptGate;
use clavyn_core::known_hosts::KnownHosts;
use clavyn_core::session::SessionManager;
use clavyn_core::sftp::SftpManager;
use clavyn_core::store::Store;
use clavyn_core::vault::{Vault, VaultKey};
use clavyn_core::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Takes the sinks lock, recovering the map if the mutex is poisoned.
///
/// Nothing but a map lookup runs under this lock, so a panic elsewhere cannot
/// leave the map half-updated and the recovered guard is always sound. Giving
/// up on a poisoned mutex instead would silently drop whichever sink the caller
/// wanted: on the close path that means never sending the frame that ends the
/// stream, leaving the pane stopped with no reason for it.
fn lock_sinks(
    sinks: &std::sync::Mutex<HashMap<String, OutputSink>>,
) -> std::sync::MutexGuard<'_, HashMap<String, OutputSink>> {
    sinks.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    pub output_sinks: OutputSinks,
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
    opening: std::collections::HashMap<String, u64>,
    next_reservation: u64,
}

/// Identifies one in-flight open. Session ids are reusable: a close drops the
/// pending reservation, and a new open can claim the same id while the first
/// shell is still starting. Keying the reservation by id alone would let the
/// earlier request's `fulfil` or `release` then consume the newer request's
/// claim, so each reservation carries a unique token that both must match.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reservation(u64);

impl LocalTerminals {
    /// Slots in use, counting terminals still being opened.
    pub fn len(&self) -> usize {
        self.live.len() + self.opening.len()
    }

    /// Whether an id is taken, whether or not its shell exists yet.
    pub fn contains(&self, session_id: &str) -> bool {
        self.live.contains_key(session_id) || self.opening.contains_key(session_id)
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

    /// Claims a session id for a terminal that has not spawned yet. The
    /// returned token is the only proof the claim is still this request's —
    /// a `fulfil` or `release` carrying an older token leaves a reservation
    /// that has since been taken over by a new open untouched.
    pub fn reserve(&mut self, session_id: String) -> Reservation {
        let reservation = Reservation(self.next_reservation);
        self.next_reservation = self.next_reservation.wrapping_add(1);
        self.opening.insert(session_id, reservation.0);
        reservation
    }

    /// Turns a reservation into a usable terminal. Returns the terminal back if
    /// the reservation is gone or now belongs to a newer open, which means the
    /// session was closed while its shell was starting and the caller has to
    /// tear it down.
    pub fn fulfil(
        &mut self,
        session_id: &str,
        reservation: Reservation,
        terminal: LocalTerminal,
    ) -> std::result::Result<(), LocalTerminal> {
        if self.opening.get(session_id) != Some(&reservation.0) {
            return Err(terminal);
        }
        self.opening.remove(session_id);
        self.live.insert(session_id.to_string(), terminal);
        Ok(())
    }

    /// Drops a reservation whose shell never started. A token that no longer
    /// matches — the request was closed and the id possibly re-reserved — is
    /// ignored rather than consuming the newer request's claim.
    pub fn release(&mut self, session_id: &str, reservation: Reservation) {
        if self.opening.get(session_id) == Some(&reservation.0) {
            self.opening.remove(session_id);
        }
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

    /// Drops every live terminal and forgets every reservation. Live terminals
    /// are disowned first so a still-running reader cannot speak for an id a
    /// later session reuses, and a reservation left behind would hold a slot
    /// for a terminal nobody wants any more.
    pub fn clear(&mut self) {
        for (_, terminal) in self.live.drain() {
            terminal.disown();
        }
        self.opening.clear();
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
    pub fn init<R: tauri::Runtime>(app: &AppHandle<R>, app_data: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&app_data).ok();
        let store = Store::load(app_data.join("store.json"))?;
        let vault = Vault::open(app_data.join("vault.json"))?;
        let known_hosts = KnownHosts::load(app_data.join("known_hosts.json"))?;

        let output_sinks: OutputSinks = Arc::new(std::sync::Mutex::new(HashMap::new()));

        let sinks = output_sinks.clone();
        let data_callback = Arc::new(move |sid: &str, data: &[u8]| {
            let sink = lock_sinks(&sinks).get(sid).cloned();
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
            let sink = lock_sinks(&sinks).remove(sid);
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
            host_key_prompt: HostKeyPromptGate::new(),
            vault_session: VaultSessionSlot::new(),
            auth_generation: AuthGeneration::new(),
            biometric_mutation: Mutex::new(()),
            sessions,
            sftp,
            output_sinks,
            local_terminals: Mutex::new(LocalTerminals::default()),
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

    /// Routes a session's output to `sink` until the session ends.
    pub fn register_output(&self, session_id: String, sink: OutputSink) {
        lock_sinks(&self.output_sinks).insert(session_id, sink);
    }

    /// Stops routing output for a session. Safe to call for a session that was
    /// never registered.
    pub fn release_output(&self, session_id: &str) {
        lock_sinks(&self.output_sinks).remove(session_id);
    }
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
        let reservation = locals.reserve(session_id.to_string());
        assert!(locals.fulfil(session_id, reservation, terminal).is_ok());
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

    /// The window the token closes: a close drops the first open's
    /// reservation, a second open claims the same id while the first shell is
    /// still starting, and the first request's `release` or `fulfil` must not
    /// consume the reservation that now belongs to the second.
    #[test]
    fn a_superseded_reservation_cannot_consume_the_newer_one() {
        let mut locals = LocalTerminals::default();
        let stale = locals.reserve("local-0".to_string());
        locals.remove("local-0");
        let current = locals.reserve("local-0".to_string());

        // A late release from the first request leaves the slot claimed.
        locals.release("local-0", stale);
        assert!(locals.contains("local-0"));

        // And its fulfil installs nothing — the caller tears the shell down.
        let (stale_terminal, _) = terminal();
        let mut refused = locals
            .fulfil("local-0", stale, stale_terminal)
            .expect_err("the reservation is no longer the first request's");
        let _ = refused.child.kill();
        assert!(locals.contains("local-0"));
        assert!(locals.get_live("local-0").is_none());

        let (live_terminal, owns) = terminal();
        assert!(locals.fulfil("local-0", current, live_terminal).is_ok());
        assert!(owns.load(Ordering::Acquire));

        let mut open = locals.remove("local-0").expect("the terminal is live");
        let _ = open.child.kill();
    }
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
