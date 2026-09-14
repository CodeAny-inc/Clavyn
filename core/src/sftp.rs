use crate::connection;
use crate::host::Host;
use crate::identity::Identity;
use crate::known_hosts::KnownHosts;
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::Handle;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// A directory entry returned by SFTP list operations.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub long_name: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<i64>,
    pub permissions: Option<u32>,
}

/// Manages SFTP sessions for multiple hosts.
pub struct SftpManager {
    sessions: Mutex<HashMap<String, SftpConnection>>,
}

struct SftpConnection {
    #[allow(dead_code)]
    handle: Arc<Mutex<Handle<connection::SshHandler>>>,
    sftp: SftpSession,
}

impl SftpManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Open an SFTP session to a host. Returns the session id.
    pub async fn connect(
        &self,
        session_id: String,
        host: &Host,
        identity: Option<&Identity>,
        known_hosts: Arc<Mutex<KnownHosts>>,
        vault: Option<&Vault>,
        passphrase: Option<&str>,
        password: Option<&str>,
    ) -> Result<()> {
        // Check if already connected
        {
            let sessions = self.sessions.lock().await;
            if sessions.contains_key(&session_id) {
                return Ok(());
            }
        }

        let handle = connection::connect(host, identity, known_hosts, vault, passphrase, password)
            .await?;

        // Open a channel and request the SFTP subsystem
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| CoreError::Ssh(format!("sftp channel open: {e}")))?;

        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| CoreError::Ssh(format!("request sftp subsystem: {e}")))?;

        // Convert the channel into an async stream for SftpSession
        let stream = channel.into_stream();
        let sftp = SftpSession::new(stream)
            .await
            .map_err(|e| CoreError::Ssh(format!("sftp init: {e}")))?;

        let handle = Arc::new(Mutex::new(handle));

        let conn = SftpConnection { handle, sftp };
        self.sessions.lock().await.insert(session_id, conn);
        Ok(())
    }

    /// List directory entries at the given path.
    pub async fn list_dir(&self, session_id: &str, path: &str) -> Result<Vec<SftpEntry>> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        let mut entries = Vec::new();
        let mut dir = conn
            .sftp
            .read_dir(path)
            .await
            .map_err(|e| CoreError::Ssh(format!("read_dir: {e}")))?;

        // ReadDir is a plain Iterator (not async)
        for entry in dir.by_ref() {
            let name = entry.file_name();
            let attrs = entry.metadata();
            let modified = attrs
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            entries.push(SftpEntry {
                name: name.clone(),
                long_name: name,
                is_dir: attrs.is_dir(),
                is_file: attrs.is_regular(),
                is_symlink: attrs.is_symlink(),
                size: attrs.len(),
                modified,
                permissions: attrs.permissions,
            });
        }

        // Sort: directories first, then files, alphabetically
        entries.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        Ok(entries)
    }

    /// Get the canonical absolute path (e.g., resolve "~").
    pub async fn canonicalize(&self, session_id: &str, path: &str) -> Result<String> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        conn.sftp
            .canonicalize(path)
            .await
            .map_err(|e| CoreError::Ssh(format!("canonicalize: {e}")))
    }

    /// Read a file's contents into a byte vector.
    ///
    /// Kept for API compatibility. Interactive file transfers should use
    /// `download_to_local` so large files never cross Tauri IPC as JSON arrays.
    pub async fn read_file(&self, session_id: &str, path: &str) -> Result<Vec<u8>> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        conn.sftp
            .read(path)
            .await
            .map_err(|e| CoreError::Ssh(format!("read file: {e}")))
    }

    /// Write data to a remote file (creates or overwrites).
    ///
    /// Kept for API compatibility. Interactive file transfers should use
    /// `upload_from_local` so large files never cross Tauri IPC as JSON arrays.
    pub async fn write_file(&self, session_id: &str, path: &str, data: &[u8]) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        conn.sftp
            .write(path, data)
            .await
            .map_err(|e| CoreError::Ssh(format!("write file: {e}")))
    }

    /// Stream a remote file directly to a local path.
    pub async fn download_to_local(
        &self,
        session_id: &str,
        remote_path: &str,
        local_path: &Path,
    ) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        let mut remote = conn
            .sftp
            .open(remote_path)
            .await
            .map_err(|e| CoreError::Ssh(format!("open remote file: {e}")))?;
        let mut local = tokio::fs::File::create(local_path).await?;

        tokio::io::copy(&mut remote, &mut local).await?;
        local.flush().await?;
        Ok(())
    }

    /// Stream a local file directly to the remote host. Existing remote files
    /// are rejected unless the caller explicitly opts in to overwrite.
    pub async fn upload_from_local(
        &self,
        session_id: &str,
        local_path: &Path,
        remote_path: &str,
        overwrite: bool,
    ) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        if !overwrite
            && conn
                .sftp
                .try_exists(remote_path)
                .await
                .map_err(|e| CoreError::Ssh(format!("check remote file: {e}")))?
        {
            return Err(CoreError::InvalidInput(format!(
                "remote file already exists: {remote_path}"
            )));
        }

        let mut local = tokio::fs::File::open(local_path).await?;
        let mut remote = conn
            .sftp
            .create(remote_path)
            .await
            .map_err(|e| CoreError::Ssh(format!("create remote file: {e}")))?;

        tokio::io::copy(&mut local, &mut remote).await?;
        remote.flush().await?;
        remote.shutdown().await?;
        Ok(())
    }

    /// Create a directory.
    pub async fn create_dir(&self, session_id: &str, path: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        conn.sftp
            .create_dir(path)
            .await
            .map_err(|e| CoreError::Ssh(format!("mkdir: {e}")))
    }

    /// Remove a file.
    pub async fn remove_file(&self, session_id: &str, path: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        conn.sftp
            .remove_file(path)
            .await
            .map_err(|e| CoreError::Ssh(format!("remove file: {e}")))
    }

    /// Remove a directory.
    pub async fn remove_dir(&self, session_id: &str, path: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        conn.sftp
            .remove_dir(path)
            .await
            .map_err(|e| CoreError::Ssh(format!("remove dir: {e}")))
    }

    /// Rename a file or directory.
    pub async fn rename(
        &self,
        session_id: &str,
        old_path: &str,
        new_path: &str,
    ) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        conn.sftp
            .rename(old_path, new_path)
            .await
            .map_err(|e| CoreError::Ssh(format!("rename: {e}")))
    }

    /// Get file metadata (size, permissions, modified time).
    pub async fn stat(&self, session_id: &str, path: &str) -> Result<SftpEntry> {
        let mut sessions = self.sessions.lock().await;
        let conn = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        let attrs = conn
            .sftp
            .metadata(path)
            .await
            .map_err(|e| CoreError::Ssh(format!("stat: {e}")))?;

        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let modified = attrs
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        Ok(SftpEntry {
            name,
            long_name: path.to_string(),
            is_dir: attrs.is_dir(),
            is_file: attrs.is_regular(),
            is_symlink: attrs.is_symlink(),
            size: attrs.len(),
            modified,
            permissions: attrs.permissions,
        })
    }

    /// Close an SFTP session.
    pub async fn close(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        if let Some(conn) = sessions.remove(session_id) {
            let _ = conn.sftp.close().await;
        }
        Ok(())
    }

    /// List active SFTP session ids.
    pub async fn list(&self) -> Vec<String> {
        self.sessions.lock().await.keys().cloned().collect()
    }

    /// Close every active SFTP session and return how many were live.
    ///
    /// The map is drained under the lock and the guard is released before any
    /// network work, so a channel stops being addressable the moment this is
    /// called even if the remote never answers the close.
    pub async fn close_all(&self) -> usize {
        let drained: Vec<SftpConnection> = {
            let mut sessions = self.sessions.lock().await;
            sessions.drain().map(|(_, conn)| conn).collect()
        };
        let count = drained.len();
        for conn in &drained {
            let _ = conn.sftp.close().await;
        }
        count
    }
}

impl Default for SftpManager {
    fn default() -> Self {
        Self::new()
    }
}
