use crate::connection;
use crate::host::Host;
use crate::known_hosts::KnownHosts;
use crate::output::OutputBatcher;
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::Handle;
use russh::ChannelMsg;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};

/// Callback invoked when a session receives data from the remote end.
/// The Tauri layer uses this to forward a batch of output to the frontend.
pub type DataCallback = Arc<dyn Fn(&str, &[u8]) + Send + Sync>;

/// Callback invoked when a session closes.
pub type CloseCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Resolves at `deadline`, or never when there is nothing to wait for.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
        None => std::future::pending().await,
    }
}

/// Manages all active terminal sessions (SSH and local).
pub struct SessionManager {
    sessions: Mutex<HashMap<String, SshSession>>,
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
/// Dropping the claim without handing it to
/// [`SessionManager::create_ssh_session`], or when that call fails, frees the
/// id again.
pub struct SessionClaim {
    id: String,
    claimed: Arc<std::sync::Mutex<HashSet<String>>>,
    committed: bool,
}

impl SessionClaim {
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl Drop for SessionClaim {
    fn drop(&mut self) {
        if !self.committed {
            lock_claims(&self.claimed).remove(&self.id);
        }
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
    #[allow(dead_code)]
    id: String,
    handle: Arc<Mutex<Handle<connection::SshHandler>>>,
    channel_id: russh::ChannelId,
    resize_tx: mpsc::Sender<(u32, u32)>,
}

// Re-export the handler type so the session manager can use it.
pub use connection::SshHandler;

impl SessionManager {
    pub fn new(data_callback: DataCallback, close_callback: CloseCallback) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
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
            committed: false,
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
        mut claim: SessionClaim,
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
        let handle =
            connection::connect(host, identity, known_hosts, vault, passphrase, password).await?;
        let channel = handle.channel_open_session().await.map_err(|e| {
            CoreError::Ssh(format!("channel open: {e}"))
        })?;
        let channel_id = channel.id();

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

        // Run startup command if set
        if let Some(cmd) = &host.startup_command {
            if !cmd.is_empty() {
                channel
                    .exec(false, cmd.clone())
                    .await
                    .map_err(|e| CoreError::Ssh(format!("startup exec: {e}")))?;
            }
        }

        let handle = Arc::new(Mutex::new(handle));
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u32, u32)>(32);

        let session = SshSession {
            id: session_id.clone(),
            handle: handle.clone(),
            channel_id,
            resize_tx,
        };

        self.sessions.lock().await.insert(session_id.clone(), session);
        // The id stays claimed for as long as the session is in the map;
        // `close` and `close_all` release it.
        claim.committed = true;

        // Spawn the reading task
        let sid = session_id.clone();
        let data_cb = self.data_callback.clone();
        let close_cb = self.close_callback.clone();
        let mut channel = channel;

        tokio::spawn(async move {
            let mut pending_resize: Option<(u32, u32)> = None;
            let mut batcher = OutputBatcher::new(Instant::now());
            let flush = |batcher: &mut OutputBatcher| {
                if !batcher.is_empty() {
                    data_cb(&sid, batcher.batch());
                    batcher.mark_flushed(Instant::now());
                }
            };
            loop {
                // Read the deadline before the select so no branch borrows the
                // batcher while another branch's handler mutates it.
                let flush_at = batcher.deadline();
                tokio::select! {
                    msg = channel.wait() => {
                        match msg {
                            Some(ChannelMsg::Data { data })
                            | Some(ChannelMsg::ExtendedData { data, .. }) => {
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
                                break;
                            }
                            _ => {}
                        }
                    }
                    resize = resize_rx.recv() => {
                        pending_resize = resize;
                    }
                    () = sleep_until(flush_at) => {
                        flush(&mut batcher);
                    }
                }
                if let Some((c, r)) = pending_resize.take() {
                    let _ = channel.window_change(c, r, 0, 0).await;
                }
            }
        });

        Ok(())
    }

    /// Write data to a session's terminal.
    pub async fn write(&self, session_id: &str, data: &[u8]) -> Result<()> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        let handle = session.handle.lock().await;
        handle
            .data(session.channel_id, data.to_vec())
            .await
            .map_err(|_| CoreError::Ssh("write failed".into()))?;
        Ok(())
    }

    /// Resize a session's terminal.
    pub async fn resize(&self, session_id: &str, cols: u32, rows: u32) -> Result<()> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        session
            .resize_tx
            .send((cols, rows))
            .await
            .map_err(|_| CoreError::Ssh("resize channel closed".into()))?;
        Ok(())
    }

    /// Close a session.
    pub async fn close(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.remove(session_id) {
            lock_claims(&self.claimed).remove(session_id);
            let handle = session.handle.lock().await;
            let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
        }
        Ok(())
    }

    /// List active session ids.
    pub async fn list(&self) -> Vec<String> {
        self.sessions.lock().await.keys().cloned().collect()
    }

    /// Close every active session and return how many were live.
    ///
    /// The map is drained under the lock and the guard is released before any
    /// network work, so a session stops being addressable the moment this is
    /// called. A remote that never acknowledges the disconnect can therefore
    /// only delay the polite goodbye, never keep a session reachable.
    pub async fn close_all(&self) -> usize {
        let drained: Vec<SshSession> = {
            let mut sessions = self.sessions.lock().await;
            let mut claimed = lock_claims(&self.claimed);
            sessions
                .drain()
                .map(|(id, session)| {
                    claimed.remove(&id);
                    session
                })
                .collect()
        };
        let count = drained.len();
        for session in &drained {
            let handle = session.handle.lock().await;
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
        count
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
            .create_ssh_session(claim, &host, None, known_hosts, None, None, None, 80, 24)
            .await
            .is_err());
        assert!(!sessions.is_claimed("s"));
    }
}
