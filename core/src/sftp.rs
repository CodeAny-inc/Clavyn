use crate::connection;
use crate::host::Host;
use crate::identity::Identity;
use crate::known_hosts::KnownHosts;
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::Handle;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio::sync::Mutex;

/// The longest a directory listing may take in total. russh-sftp reads a
/// directory until the server reports its end, with a timeout on each request
/// but none overall, so a server that keeps answering with more entries
/// would otherwise keep the listing, and its memory, growing for good.
const LIST_DIR_DEADLINE: Duration = Duration::from_secs(30);

/// How long a polite goodbye may take before the connection is dropped.
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// Read buffer for uploads. russh-sftp keeps up to 16 writes in flight, each
/// as large as the slice it is handed, so the read size sets how much data is
/// in the air per round trip: `tokio::io::copy`'s 8 KiB would cap it at
/// 128 KiB, far below what a long link can carry.
const UPLOAD_BUFFER: usize = 256 * 1024;

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
///
/// The session map is locked only to look an entry up, insert or remove it,
/// never across network I/O. A transfer or a listing therefore holds up
/// nothing but itself: other sessions keep working, and `close` and
/// `close_all` can always take an entry out of reach at once.
pub struct SftpManager {
    sessions: Mutex<HashMap<String, Arc<SftpConnection>>>,
}

struct SftpConnection {
    handle: Handle<connection::SshHandler>,
    sftp: SftpSession,
    /// Flipped once the connection is closed. Every operation watches it and
    /// stops, so one that is stuck behind a peer that stopped reading, or
    /// simply still running, cannot outlive the close or report success
    /// after it.
    closed: tokio::sync::watch::Sender<bool>,
}

impl SftpConnection {
    /// Run `operation` until it finishes or the connection is closed.
    async fn until_closed<T>(
        &self,
        operation: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let mut closed = self.closed.subscribe();
        tokio::select! {
            result = operation => result,
            _ = closed.wait_for(|closed| *closed) => {
                Err(CoreError::Ssh("the SFTP session was closed".into()))
            }
        }
    }

    /// Stop every request, listing and transfer on this connection, end the
    /// SFTP channel, then the connection. Bounded, so an unresponsive peer cannot
    /// hold the caller.
    async fn close(&self) {
        self.closed.send_replace(true);
        let _ = tokio::time::timeout(CLOSE_GRACE, async {
            let _ = self.sftp.close().await;
            let _ = self
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        })
        .await;
    }
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
        // An id already in use is refused rather than reported as connected.
        // Returning Ok here would have the caller's later list_dir, remove_file
        // or rename run against whichever host holds the id — the same
        // collision the check below refuses, answered differently only because
        // the other connect happened to finish first.
        {
            let sessions = self.sessions.lock().await;
            if sessions.contains_key(&session_id) {
                return Err(CoreError::InvalidInput(
                    "session id is already in use by an SFTP session".into(),
                ));
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

        let conn = Arc::new(SftpConnection {
            handle,
            sftp,
            closed: tokio::sync::watch::Sender::new(false),
        });
        // The check at the top ran before a connect that takes seconds, so a
        // second connect on the same id may have finished in the meantime. The
        // session already published keeps the id; this one is dropped rather
        // than silently replacing it.
        match self.sessions.lock().await.entry(session_id) {
            std::collections::hash_map::Entry::Occupied(_) => Err(CoreError::InvalidInput(
                "session id is already in use by an SFTP session".into(),
            )),
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(conn);
                Ok(())
            }
        }
    }

    /// The live connection behind `session_id`, taken out from under the map
    /// lock.
    async fn get(&self, session_id: &str) -> Result<Arc<SftpConnection>> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    /// List directory entries at the given path.
    pub async fn list_dir(&self, session_id: &str, path: &str) -> Result<Vec<SftpEntry>> {
        let conn = self.get(session_id).await?;

        let mut entries = Vec::new();
        let mut dir = conn
            .until_closed(async {
                tokio::time::timeout(LIST_DIR_DEADLINE, conn.sftp.read_dir(path))
                    .await
                    .map_err(|_| {
                        CoreError::Ssh(format!(
                            "read_dir: the listing took longer than {}s",
                            LIST_DIR_DEADLINE.as_secs()
                        ))
                    })?
                    .map_err(|e| CoreError::Ssh(format!("read_dir: {e}")))
            })
            .await?;

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
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            conn.sftp
                .canonicalize(path)
                .await
                .map_err(|e| CoreError::Ssh(format!("canonicalize: {e}")))
        })
        .await
    }

    /// Read a file's contents into a byte vector.
    ///
    /// Kept for API compatibility. Interactive file transfers should use
    /// `download_to_local` so large files never cross Tauri IPC as JSON arrays.
    pub async fn read_file(&self, session_id: &str, path: &str) -> Result<Vec<u8>> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            conn.sftp
                .read(path)
                .await
                .map_err(|e| CoreError::Ssh(format!("read file: {e}")))
        })
        .await
    }

    /// Write data to a remote file (creates or overwrites).
    ///
    /// Kept for API compatibility. Interactive file transfers should use
    /// `upload_from_local` so large files never cross Tauri IPC as JSON arrays.
    pub async fn write_file(&self, session_id: &str, path: &str, data: &[u8]) -> Result<()> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            conn.sftp
                .write(path, data)
                .await
                .map_err(|e| CoreError::Ssh(format!("write file: {e}")))
        })
        .await
    }

    /// Stream a remote file directly to a local path. An existing local file is
    /// refused unless the caller explicitly opts in to overwrite, mirroring
    /// `upload_from_local`.
    pub async fn download_to_local(
        &self,
        session_id: &str,
        remote_path: &str,
        local_path: &Path,
        overwrite: bool,
    ) -> Result<()> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            let mut remote = conn
                .sftp
                .open(remote_path)
                .await
                .map_err(|e| CoreError::Ssh(format!("open remote file: {e}")))?;
            write_download(&mut remote, local_path, overwrite).await
        })
        .await
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
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
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

            let mut local = tokio::io::BufReader::with_capacity(
                UPLOAD_BUFFER,
                tokio::fs::File::open(local_path).await?,
            );
            let mut remote = conn
                .sftp
                .create(remote_path)
                .await
                .map_err(|e| CoreError::Ssh(format!("create remote file: {e}")))?;

            tokio::io::copy_buf(&mut local, &mut remote).await?;
            remote.flush().await?;
            remote.shutdown().await?;
            Ok(())
        })
        .await
    }

    /// Create a directory.
    pub async fn create_dir(&self, session_id: &str, path: &str) -> Result<()> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            conn.sftp
                .create_dir(path)
                .await
                .map_err(|e| CoreError::Ssh(format!("mkdir: {e}")))
        })
        .await
    }

    /// Remove a file.
    pub async fn remove_file(&self, session_id: &str, path: &str) -> Result<()> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            conn.sftp
                .remove_file(path)
                .await
                .map_err(|e| CoreError::Ssh(format!("remove file: {e}")))
        })
        .await
    }

    /// Remove a directory.
    pub async fn remove_dir(&self, session_id: &str, path: &str) -> Result<()> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            conn.sftp
                .remove_dir(path)
                .await
                .map_err(|e| CoreError::Ssh(format!("remove dir: {e}")))
        })
        .await
    }

    /// Rename a file or directory.
    pub async fn rename(
        &self,
        session_id: &str,
        old_path: &str,
        new_path: &str,
    ) -> Result<()> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            conn.sftp
                .rename(old_path, new_path)
                .await
                .map_err(|e| CoreError::Ssh(format!("rename: {e}")))
        })
        .await
    }

    /// Get file metadata (size, permissions, modified time).
    pub async fn stat(&self, session_id: &str, path: &str) -> Result<SftpEntry> {
        let conn = self.get(session_id).await?;

        let attrs = conn
            .until_closed(async {
                conn.sftp
                    .metadata(path)
                    .await
                    .map_err(|e| CoreError::Ssh(format!("stat: {e}")))
            })
            .await?;

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

    /// Close an SFTP session. The entry leaves the map before anything is
    /// sent, and closing the channel ends any transfer still running on it.
    pub async fn close(&self, session_id: &str) -> Result<()> {
        let removed = self.sessions.lock().await.remove(session_id);
        if let Some(conn) = removed {
            conn.close().await;
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
    /// called even if the remote never answers the close. Transfers and
    /// listings never hold the lock, so the drain does not wait on them.
    pub async fn close_all(&self) -> usize {
        let drained: Vec<Arc<SftpConnection>> = {
            let mut sessions = self.sessions.lock().await;
            sessions.drain().map(|(_, conn)| conn).collect()
        };
        let count = drained.len();
        let goodbyes: Vec<_> = drained
            .into_iter()
            .map(|conn| tokio::spawn(async move { conn.close().await }))
            .collect();
        for goodbye in goodbyes {
            let _ = goodbye.await;
        }
        count
    }
}

impl Default for SftpManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Write a download to `target` without ever leaving a partial file behind.
///
/// Overwriting goes through a temporary file beside the target that is
/// renamed over it only once every byte is on disk, so a dropped connection
/// or a server timeout leaves the user's existing file as it was. A new file
/// is written in place and removed again if the transfer fails. Either way a
/// failed download leaves nothing that looks like a finished one, and that
/// holds when the transfer is cancelled too, as closing its session does.
async fn write_download<R: AsyncRead + Unpin + ?Sized>(
    source: &mut R,
    target: &Path,
    overwrite: bool,
) -> Result<()> {
    let staging = if overwrite {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Some(target.with_file_name(format!(".{name}.{}.part", uuid::Uuid::new_v4())))
    } else {
        None
    };
    let written_to = staging.as_deref().unwrap_or(target);
    // The staging name is fresh, so `create_new` holds for it as well.
    let file = open_download_target(written_to, false).await?;
    let mut partial = PartialDownload {
        path: written_to.to_path_buf(),
        file: Some(file),
    };
    let file = partial.file.as_mut().expect("just opened");
    tokio::io::copy(source, file).await?;
    file.flush().await?;
    file.sync_all().await?;
    partial.file = None;
    if let Some(staging) = &staging {
        tokio::fs::rename(staging, target).await?;
    }
    partial.path = PathBuf::new();
    Ok(())
}

/// A download that has not finished. Dropped before it is complete, whether
/// by an error or because the transfer future itself was dropped, it removes
/// the file written so far.
struct PartialDownload {
    /// Emptied once the download is in place.
    path: PathBuf,
    file: Option<tokio::fs::File>,
}

impl Drop for PartialDownload {
    fn drop(&mut self) {
        // The handle goes first: Windows refuses to remove a file that is
        // still open without delete sharing.
        self.file = None;
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Open the local file a download writes into. Without `overwrite` an existing
/// file is refused; `create_new` checks and creates in one step, so a file that
/// appears between a check and the open is refused too.
async fn open_download_target(path: &Path, overwrite: bool) -> Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    options.open(path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            CoreError::InvalidInput(format!("local file already exists: {}", path.display()))
        } else {
            e.into()
        }
    })
}

#[cfg(test)]
mod download_target_tests {
    use super::{open_download_target, write_download};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};

    /// Yields `data`, then fails the way a dropped connection does.
    struct BrokenAfter(&'static [u8]);

    impl AsyncRead for BrokenAfter {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.0.is_empty() {
                return Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()));
            }
            let n = self.0.len().min(buf.remaining());
            buf.put_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Poll::Ready(Ok(()))
        }
    }

    fn entries(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn an_overwrite_replaces_the_file_only_once_complete() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");

        write_download(&mut &b"replacement"[..], &path, true)
            .await
            .expect("download");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "replacement");
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }

    #[tokio::test]
    async fn a_failed_overwrite_keeps_the_original_and_no_partial_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");

        assert!(write_download(&mut BrokenAfter(b"half of the"), &path, true)
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }

    #[tokio::test]
    async fn a_failed_new_download_leaves_nothing_behind() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");

        assert!(write_download(&mut BrokenAfter(b"half of the"), &path, false)
            .await
            .is_err());
        assert!(entries(dir.path()).is_empty());
    }

    /// Yields `data`, then never answers again, like a peer that went quiet.
    struct StallsAfter(&'static [u8]);

    impl AsyncRead for StallsAfter {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.0.is_empty() {
                return Poll::Pending;
            }
            let n = self.0.len().min(buf.remaining());
            buf.put_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_cancelled_download_leaves_the_original_and_nothing_else() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");

        let mut source = StallsAfter(b"half of the");
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            write_download(&mut source, &path, true),
        )
        .await;
        assert!(cancelled.is_err(), "the stalled download finished");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");
        assert_eq!(entries(dir.path()), ["report.txt"]);

        let fresh = dir.path().join("new.txt");
        let mut source = StallsAfter(b"half of the");
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            write_download(&mut source, &fresh, false),
        )
        .await;
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }

    #[tokio::test]
    async fn a_download_without_overwrite_keeps_an_existing_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");

        assert!(write_download(&mut &b"replacement"[..], &path, false)
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");
    }

    #[tokio::test]
    async fn an_existing_file_is_kept_unless_overwrite_is_chosen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");

        assert!(open_download_target(&path, false).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");

        let mut file = open_download_target(&path, true).await.expect("overwrite");
        file.write_all(b"new").await.expect("write");
        file.flush().await.expect("flush");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "new");
    }

    #[tokio::test]
    async fn a_new_file_is_created_either_way() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(open_download_target(&dir.path().join("a"), false).await.is_ok());
        assert!(open_download_target(&dir.path().join("b"), true).await.is_ok());
    }
}
