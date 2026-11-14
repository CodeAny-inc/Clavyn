use crate::connection;
use crate::host::Host;
use crate::known_hosts::KnownHosts;
use crate::output::{sleep_until, OutputBatcher, OutputCredit};
use crate::transport::{self, CloseSignal, Transport};
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::Handle;
use russh::ChannelMsg;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex};

/// How long the shell's output has to pause before a startup command is typed.
///
/// The login banner arrives before the shell has run its startup files, and a
/// shell still running them can read typed input meant for its prompt or have
/// the terminal echo it ahead of the prompt. A shell prints its prompt last and
/// then waits, so output that has started and then stopped for this long is
/// taken as the prompt. It is longer than a typical interactive shell takes
/// between the banner and its first prompt.
const STARTUP_SETTLE: Duration = Duration::from_secs(1);

/// Upper bound on waiting for the shell before the startup command is typed
/// anyway, so a shell that prints nothing, or never stops printing, still gets
/// it. Input sent in the meantime waits behind the command for at most this
/// long.
const STARTUP_WAIT_LIMIT: Duration = Duration::from_secs(5);

type SessionMap = HashMap<String, Arc<SshSession>>;

/// Callback invoked when a session receives data from the remote end.
/// The Tauri layer uses this to forward a batch of output to the frontend.
pub type DataCallback = Arc<dyn Fn(&str, &[u8]) + Send + Sync>;

/// Callback invoked when a session closes.
pub type CloseCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Manages all active terminal sessions (SSH and local).
pub struct SessionManager {
    /// Live sessions by id. Held only to look an entry up, insert or remove
    /// it, never across network I/O: one peer that stops reading would
    /// otherwise stall writes, resizes and closes on every other session, and
    /// keep a vault reset from draining the map.
    sessions: Arc<Mutex<SessionMap>>,
    /// Every id held by an SSH session, from the moment it is claimed — before
    /// the connection exists — until the session is closed. `sessions` only
    /// learns an id once the connection is up, seconds later, so it cannot tell
    /// whether an id is free. A plain mutex, never held across an await, so
    /// callers can consult it while holding their own locks.
    claimed: Arc<std::sync::Mutex<HashSet<String>>>,
    data_callback: DataCallback,
    close_callback: CloseCallback,
}

/// An SSH session id, reserved for a session that is about to connect.
///
/// Dropping the claim frees the id again, unless the session was started, in
/// which case the id passes to the [`ClaimedId`] guards that outlive this.
/// Callers therefore keep the claim until they have finished unwinding a failed
/// connect: an id freed earlier than that can be taken by another connect whose
/// output sink the unwinding caller then removes.
pub struct SessionClaim {
    id: String,
    claimed: Arc<std::sync::Mutex<HashSet<String>>>,
    committed: std::sync::atomic::AtomicBool,
}

impl SessionClaim {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Hand the id over to a guard that holds it for as long as anything can
    /// still act under it, and stop this claim from freeing it.
    fn commit(&self) -> Arc<ClaimedId> {
        self.committed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Arc::new(ClaimedId {
            id: self.id.clone(),
            claimed: self.claimed.clone(),
        })
    }
}

impl Drop for SessionClaim {
    fn drop(&mut self) {
        if !self.committed.load(std::sync::atomic::Ordering::Relaxed) {
            lock_claims(&self.claimed).remove(&self.id);
        }
    }
}

/// Holds a session id for as long as anything can still act under it.
///
/// One of these is shared by the map entry and the reader task, and the id is
/// released only when both are gone. Closing a session deliberately leaves the
/// reader running so it can flush the trailing output and announce the close,
/// and both of those carry nothing but the id — so freeing the id when the map
/// entry goes would let a new session take it and then have the old reader
/// render its tail in the new pane, remove the new pane's output sink and
/// report the new session as closed.
struct ClaimedId {
    id: String,
    claimed: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl Drop for ClaimedId {
    fn drop(&mut self) {
        lock_claims(&self.claimed).remove(&self.id);
    }
}

/// A poisoned set still holds valid ids; a panic elsewhere is no reason to
/// stop tracking them.
fn lock_claims(
    claimed: &std::sync::Mutex<HashSet<String>>,
) -> std::sync::MutexGuard<'_, HashSet<String>> {
    claimed.lock().unwrap_or_else(|e| e.into_inner())
}

struct SshSession {
    /// Shared with the goodbye sent after the session is closed, which needs
    /// nothing else from it, so that goodbye does not keep the id claimed.
    handle: Arc<Handle<connection::SshHandler>>,
    transport: Transport,
    /// Raised once the session is closed. A write stuck behind a peer that
    /// stopped reading watches it and gives up at once rather than when the
    /// goodbye ends the connection.
    closed: CloseSignal,
    channel_id: russh::ChannelId,
    /// Serializes writes to this session so keystrokes reach the channel in
    /// the order they were sent. Per session, so a stalled peer only holds up
    /// its own input. Shared so a startup command can hold its turn while it
    /// waits for the shell.
    writes: Arc<Mutex<()>>,
    /// The size the remote terminal should have. Only the latest one matters,
    /// so a resize replaces it and never waits; the session's resizer task
    /// sends it on whenever the connection takes it.
    size: watch::Sender<(u32, u32)>,
    /// Output delivered to the UI layer and not yet acknowledged, shared with
    /// the reader task, which stops reading the channel while it is spent.
    credit: Arc<OutputCredit>,
    /// Held so the id stays reserved while this entry exists.
    #[allow(dead_code)]
    claim: Arc<ClaimedId>,
}

impl SshSession {
    /// Send `data` to this session's terminal.
    ///
    /// The send waits for as long as the peer leaves the connection unread,
    /// and only this session's writes wait behind it. A close ends the wait.
    async fn send(&self, session_id: &str, data: &[u8]) -> Result<()> {
        let send = async {
            let _order = self.writes.lock().await;
            self.handle
                .data(self.channel_id, data.to_vec())
                .await
                .map_err(|_| CoreError::Ssh("write failed".into()))
        };
        self.closed
            .guard(send, || CoreError::SessionNotFound(session_id.to_string()))
            .await
    }

    /// Mark the session closed and say goodbye to the peer, cutting the
    /// connection if the goodbye does not get through in time. The goodbye
    /// holds only the handle and the transport, so the id is released as soon
    /// as the map entry and the reader are gone, and the reader goes once the
    /// connection does, so within [`transport::GOODBYE_GRACE`] at most.
    ///
    /// The output limit is lifted as well: a reader stalled on unacknowledged
    /// output has to keep reading to reach the end of the channel, flush the
    /// tail and announce the close.
    fn retire(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        self.closed.raise();
        self.credit.lift();
        let handle = self.handle.clone();
        let transport = self.transport.clone();
        async move { transport::goodbye(&handle, &transport).await }
    }
}

// Re-export the handler type so the session manager can use it.
pub use connection::SshHandler;

impl SessionManager {
    pub fn new(data_callback: DataCallback, close_callback: CloseCallback) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            claimed: Arc::new(std::sync::Mutex::new(HashSet::new())),
            data_callback,
            close_callback,
        }
    }

    /// Reserve `session_id` for an SSH session, or refuse it when an SSH session
    /// already holds it, connected or still connecting.
    ///
    /// Session ids are the only address `write`, `resize` and the output stream
    /// use, so two sessions on one id would each receive the other's traffic.
    /// The check and the reservation are one step under one lock, so of two
    /// concurrent claims on the same id exactly one succeeds.
    pub fn claim(&self, session_id: &str) -> Result<SessionClaim> {
        if !lock_claims(&self.claimed).insert(session_id.to_string()) {
            return Err(CoreError::InvalidInput(
                "session id is already in use by an SSH session".into(),
            ));
        }
        Ok(SessionClaim {
            id: session_id.to_string(),
            claimed: self.claimed.clone(),
            committed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Whether an SSH session holds `session_id`, connected or still
    /// connecting.
    pub fn is_claimed(&self, session_id: &str) -> bool {
        lock_claims(&self.claimed).contains(session_id)
    }

    /// Connect to a host, open a channel, request PTY + shell, and start
    /// streaming data on the id `claim` reserved.
    pub async fn create_ssh_session(
        &self,
        claim: &SessionClaim,
        host: &Host,
        identity: Option<&crate::identity::Identity>,
        known_hosts: Arc<Mutex<KnownHosts>>,
        vault: Option<&Vault>,
        passphrase: Option<&str>,
        password: Option<&str>,
        cols: u32,
        rows: u32,
    ) -> Result<()> {
        let session_id = claim.id.clone();
        let connection::Connection { handle, transport } =
            connection::connect(host, identity, known_hosts, vault, passphrase, password).await?;
        let opened = async {
            let channel = handle
                .channel_open_session()
                .await
                .map_err(|e| CoreError::Ssh(format!("channel open: {e}")))?;

            // Request PTY
            channel
                .request_pty(
                    false,
                    "xterm-256color",
                    cols,
                    rows,
                    0,
                    0,
                    &[], // default terminal modes
                )
                .await
                .map_err(|e| CoreError::Ssh(format!("request pty: {e}")))?;

            // Request shell
            channel
                .request_shell(false)
                .await
                .map_err(|e| CoreError::Ssh(format!("request shell: {e}")))?;
            Ok::<_, CoreError>(channel)
        };
        let channel = match opened.await {
            Ok(channel) => channel,
            Err(e) => {
                transport.cut();
                return Err(e);
            }
        };
        let channel_id = channel.id();
        // The reader keeps only the read half, so nothing it does waits on the
        // connection taking outbound messages; it always drains what arrives.
        let (mut reader, writer) = channel.split();
        let (size, mut size_rx) = watch::channel((cols, rows));
        let credit = Arc::new(OutputCredit::new());

        let held = claim.commit();
        let session = Arc::new(SshSession {
            handle: Arc::new(handle),
            transport,
            closed: CloseSignal::new(),
            channel_id,
            writes: Arc::new(Mutex::new(())),
            size,
            credit: credit.clone(),
            claim: held.clone(),
        });

        // A startup command takes the first turn to write before the session
        // is reachable, so nothing written to it can overtake the command. The
        // reader reports each batch of output for as long as it waits.
        let (mut output_seen, startup) =
            match host.startup_command.as_deref().filter(|cmd| !cmd.is_empty()) {
                Some(command) => {
                    let (seen, output) = watch::channel(());
                    let startup = StartupCommand {
                        command: command.to_string(),
                        turn: session.writes.clone().lock_owned().await,
                        output,
                    };
                    (Some(seen), Some(startup))
                }
                None => (None, None),
            };

        self.sessions
            .lock()
            .await
            .insert(session_id.clone(), session.clone());
        let sessions = self.sessions.clone();

        // Window changes wait whenever the connection does, so they go out from
        // a task of their own. It ends once the entry, and the sender with it,
        // is gone, or once the connection is.
        tokio::spawn(async move {
            while size_rx.changed().await.is_ok() {
                let (cols, rows) = *size_rx.borrow_and_update();
                if writer.window_change(cols, rows, 0, 0).await.is_err() {
                    break;
                }
            }
        });

        // Spawn the reading task
        let sid = session_id.clone();
        let data_cb = self.data_callback.clone();
        let close_cb = self.close_callback.clone();

        tokio::spawn(async move {
            // Dropped when this task ends, after the close has been announced,
            // which is the moment the id is free for another session.
            let held = held;
            let mut batcher = OutputBatcher::new(Instant::now());
            let flush = |batcher: &mut OutputBatcher| {
                if !batcher.is_empty() {
                    let batch = batcher.batch();
                    // Spent before the batch is handed over, so an
                    // acknowledgement can never arrive ahead of the spend
                    // it pays back.
                    credit.spend(batch.len());
                    data_cb(&sid, batch);
                    batcher.mark_flushed(Instant::now());
                }
            };
            loop {
                // Read the deadline before the select so no branch borrows the
                // batcher while another branch's handler mutates it.
                let flush_at = batcher.deadline();
                // While the UI layer is a window behind, the channel is left
                // unread, which backs the stall up to the peer. A batch already
                // due is still delivered meanwhile; the end of the stream is
                // seen once credit returns or the session is closed, which
                // lifts the limit. Nothing in this loop waits on the connection
                // taking outbound messages, which russh's connection loop
                // cannot send while it is blocked handing this reader data.
                let reading = credit.has_room();
                tokio::select! {
                    () = credit.room(), if !reading => {}
                    msg = reader.wait(), if reading => {
                        match msg {
                            Some(ChannelMsg::Data { data })
                            | Some(ChannelMsg::ExtendedData { data, .. }) => {
                                // Nobody listens once the startup command has
                                // stopped waiting, and the sender goes with it.
                                if output_seen.as_ref().is_some_and(|seen| seen.send(()).is_err()) {
                                    output_seen = None;
                                }
                                if batcher.push(&data, Instant::now()) {
                                    flush(&mut batcher);
                                }
                            }
                            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                                // Trailing output belongs to this session, so it
                                // is handed over before the close is announced.
                                // The UI transport is what keeps the two in
                                // order; see `AppState::init`.
                                flush(&mut batcher);
                                close_cb(&sid, "session closed");
                                // A shell that exits leaves the connection up;
                                // end it, unless a close already took the entry.
                                // The goodbye runs on its own so the id is free
                                // for a reconnect as soon as this task ends.
                                if let Some(session) = take_if_held(&sessions, &sid, &held).await {
                                    tokio::spawn(session.retire());
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                    () = sleep_until(flush_at) => {
                        flush(&mut batcher);
                    }
                }
            }
        });

        // A channel takes only one shell, exec or subsystem request, and
        // OpenSSH ignores an exec that arrives after the shell, so the startup
        // command is typed into the shell instead. It waits for the shell on a
        // task of its own, so the connect does not, and the session is usable
        // whether or not the command goes out.
        if let Some(startup) = startup {
            tokio::spawn(type_startup_command(session, session_id, startup));
        }

        Ok(())
    }

    /// The live session behind `session_id`, taken out from under the map lock.
    async fn get(&self, session_id: &str) -> Result<Arc<SshSession>> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    /// Whether an SSH session is live under `session_id`.
    pub async fn contains(&self, session_id: &str) -> bool {
        self.sessions.lock().await.contains_key(session_id)
    }

    /// Write data to a session's terminal.
    ///
    /// The send waits for as long as the peer leaves the connection unread,
    /// and only this session's writes wait behind it. A close ends the wait.
    pub async fn write(&self, session_id: &str, data: &[u8]) -> Result<()> {
        self.get(session_id).await?.send(session_id, data).await
    }

    /// Resize a session's terminal. Never waits on the peer: the new size
    /// replaces any that has not gone out yet.
    pub async fn resize(&self, session_id: &str, cols: u32, rows: u32) -> Result<()> {
        self.get(session_id).await?.size.send_replace((cols, rows));
        Ok(())
    }

    /// Close a session.
    ///
    /// The entry leaves the map before the goodbye is sent, so the session
    /// stops being addressable at once even if the peer never answers, and the
    /// entry's share of the id goes with it rather than waiting on the
    /// goodbye. The reader task still holds a share until it has flushed and
    /// announced the close, which the goodbye guarantees within
    /// [`transport::GOODBYE_GRACE`]. Output credit is lifted first, so a reader
    /// waiting on the UI layer goes on to the end of the stream instead of
    /// waiting on acknowledgements that may never come.
    pub async fn close(&self, session_id: &str) -> Result<()> {
        let removed = self.sessions.lock().await.remove(session_id);
        if let Some(session) = removed {
            let goodbye = session.retire();
            drop(session);
            goodbye.await;
        }
        Ok(())
    }

    /// Return `bytes` of output credit to a session once the UI layer has
    /// received them, letting a reader that ran out resume. Answers whether an
    /// SSH session took them.
    ///
    /// An id with no live session is not an error: acknowledgements trail the
    /// output they cover and can arrive after the session has closed.
    pub async fn ack(&self, session_id: &str, bytes: u64) -> bool {
        match self.get(session_id).await {
            Ok(session) => {
                session.credit.refund(bytes);
                true
            }
            Err(_) => false,
        }
    }

    /// List active session ids.
    pub async fn list(&self) -> Vec<String> {
        self.sessions.lock().await.keys().cloned().collect()
    }

    /// Close every active session and return how many were live.
    ///
    /// The map is drained under the lock and the guard is released before any
    /// network work, so a session stops being addressable the moment this is
    /// called. The goodbyes then run side by side, each bounded by
    /// [`transport::GOODBYE_GRACE`] and ending its connection either way, so
    /// one peer that never answers cannot hold up the others or outlive the
    /// call.
    pub async fn close_all(&self) -> usize {
        let drained: Vec<Arc<SshSession>> = {
            let mut sessions = self.sessions.lock().await;
            sessions.drain().map(|(_, session)| session).collect()
        };
        let count = drained.len();
        let goodbyes: Vec<_> = drained
            .into_iter()
            .map(|session| tokio::spawn(session.retire()))
            .collect();
        for goodbye in goodbyes {
            let _ = goodbye.await;
        }
        count
    }
}

/// Remove `session_id` from the map if the entry there is still the session
/// `held` belongs to. An id is reused once its session has ended, and a
/// reader finishing late must not take a newer session's entry.
async fn take_if_held(
    sessions: &Mutex<SessionMap>,
    session_id: &str,
    held: &Arc<ClaimedId>,
) -> Option<Arc<SshSession>> {
    let mut sessions = sessions.lock().await;
    match sessions.get(session_id) {
        Some(session) if Arc::ptr_eq(&session.claim, held) => sessions.remove(session_id),
        _ => None,
    }
}

/// A host's startup command, waiting for its shell to be ready for input.
struct StartupCommand {
    command: String,
    /// The session's first turn to write. Input sent while the command waits
    /// queues behind it and reaches the shell after the command, in order.
    turn: tokio::sync::OwnedMutexGuard<()>,
    /// Marked by the reader each time the shell prints something; closed when
    /// the reader ends.
    output: watch::Receiver<()>,
}

/// Whether the shell is ready for typed input: `true` once its output has
/// started and then paused for [`STARTUP_SETTLE`], or once
/// [`STARTUP_WAIT_LIMIT`] has passed, and `false` if the output ends first,
/// which means the session is ending.
async fn shell_settled(mut output: watch::Receiver<()>) -> bool {
    let settled = async {
        if output.changed().await.is_err() {
            return false;
        }
        loop {
            match tokio::time::timeout(STARTUP_SETTLE, output.changed()).await {
                Err(_) => return true,
                Ok(Ok(())) => {}
                Ok(Err(_)) => return false,
            }
        }
    };
    tokio::time::timeout(STARTUP_WAIT_LIMIT, settled)
        .await
        .unwrap_or(true)
}

/// Type `startup.command` into the session's shell once [`shell_settled`]
/// says the shell is ready, followed by `\r`, the byte Enter sends.
///
/// It goes to `session` itself, not to whatever the id names by then: a shell
/// that exits at once frees the id, and a session that takes it next must
/// never receive this one's input. A close ends the wait and releases the
/// write turn.
async fn type_startup_command(session: Arc<SshSession>, session_id: String, startup: StartupCommand) {
    let StartupCommand {
        command,
        turn,
        output,
    } = startup;
    let typed = async {
        let _turn = turn;
        if !shell_settled(output).await {
            return Ok(());
        }
        session
            .handle
            .data(session.channel_id, format!("{command}\r").into_bytes())
            .await
            .map_err(|_| CoreError::Ssh("the connection is gone".into()))
    };
    let closed = || CoreError::SessionNotFound(session_id.clone());
    if let Err(CoreError::Ssh(reason)) = session.closed.guard(typed, closed).await {
        tracing::warn!("startup command for session {session_id} was not sent: {reason}");
    }
}

#[cfg(test)]
mod claim_tests {
    use super::SessionManager;
    use std::sync::Arc;

    fn manager() -> SessionManager {
        SessionManager::new(Arc::new(|_, _| {}), Arc::new(|_, _| {}))
    }

    #[test]
    fn an_id_can_be_claimed_once() {
        let sessions = manager();
        let claim = sessions.claim("s").expect("first claim");
        assert_eq!(claim.id(), "s");
        assert!(sessions.is_claimed("s"));
        let error = sessions.claim("s").err().expect("second claim refused");
        assert!(error.to_string().contains("already in use"), "{error}");
        assert!(sessions.claim("other").is_ok());
    }

    #[test]
    fn an_unused_claim_frees_its_id_when_dropped() {
        let sessions = manager();
        drop(sessions.claim("s").expect("claim"));
        assert!(!sessions.is_claimed("s"));
        assert!(sessions.claim("s").is_ok());
    }

    /// A connect that fails consumes its claim, and the id is free again, so a
    /// retry on the same pane is not refused.
    #[tokio::test]
    async fn a_failed_connect_frees_its_id() {
        let sessions = manager();
        let host: crate::host::Host = serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "label": "Fixture",
            "hostname": "must-not-connect.example.test",
            "port": 22,
            "username": "deploy",
            "auth": "agent",
            "tags": []
        }))
        .expect("host");
        let known_hosts = std::env::temp_dir().join(format!(
            "clavyn-claim-test-{}.json",
            uuid::Uuid::new_v4()
        ));
        let known_hosts = Arc::new(tokio::sync::Mutex::new(
            crate::known_hosts::KnownHosts::load(known_hosts).expect("known hosts"),
        ));

        let claim = sessions.claim("s").expect("claim");
        // Agent auth is refused before any network work, so this fails fast.
        assert!(sessions
            .create_ssh_session(&claim, &host, None, known_hosts, None, None, None, 80, 24)
            .await
            .is_err());
        // The caller still holds the claim: it unwinds its own output sink
        // first, and the id must not be takeable until it has.
        assert!(sessions.is_claimed("s"));
        drop(claim);
        assert!(!sessions.is_claimed("s"));
    }
}
