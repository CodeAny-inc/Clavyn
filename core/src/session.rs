use crate::connection;
use crate::host::Host;
use crate::known_hosts::KnownHosts;
use crate::output::OutputBatcher;
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::Handle;
use russh::ChannelMsg;
use russh_cryptovec::CryptoVec;
use std::collections::HashMap;
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
    data_callback: DataCallback,
    close_callback: CloseCallback,
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
            data_callback,
            close_callback,
        }
    }

    /// Connect to a host, open a channel, request PTY + shell, and start
    /// streaming data. Returns the session id.
    pub async fn create_ssh_session(
        &self,
        session_id: String,
        host: &Host,
        identity: Option<&crate::identity::Identity>,
        known_hosts: Arc<Mutex<KnownHosts>>,
        vault: Option<&Vault>,
        passphrase: Option<&str>,
        password: Option<&str>,
        cols: u32,
        rows: u32,
    ) -> Result<()> {
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
            .data(session.channel_id, CryptoVec::from(data.to_vec()))
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
            let handle = session.handle.lock().await;
            let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
        }
        Ok(())
    }

    /// List active session ids.
    pub async fn list(&self) -> Vec<String> {
        self.sessions.lock().await.keys().cloned().collect()
    }
}
