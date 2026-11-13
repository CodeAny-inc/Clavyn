use crate::connection;
use crate::host::Host;
use crate::identity::Identity;
use crate::known_hosts::KnownHosts;
use crate::transport::{self, CloseSignal, Transport};
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::Handle;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{RawSftpSession, SftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// The most entries one listing returns. A server that keeps answering a
/// listing with more entries would otherwise keep it, and its memory, growing
/// for good; a directory past this is refused rather than shown in part.
const MAX_LIST_ENTRIES: usize = 200_000;

/// The most name bytes one listing collects, for the same reason: entry
/// names are not bounded by the protocol, so the entry cap alone does not
/// bound the memory.
const MAX_LIST_NAME_BYTES: usize = 32 * 1024 * 1024;

/// The longest a listing may take in total on a server without a listing
/// channel of its own; see [`SftpConnection::lister`].
const FALLBACK_LIST_DEADLINE: Duration = Duration::from_secs(30);

/// How long opening the listing channel may take before the connection does
/// without it.
const LISTER_OPEN_TIMEOUT: Duration = Duration::from_secs(10);

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

impl SftpEntry {
    fn new(name: String, long_name: String, attrs: &FileAttributes) -> Self {
        let modified = attrs
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        Self {
            name,
            long_name,
            is_dir: attrs.is_dir(),
            is_file: attrs.is_regular(),
            is_symlink: attrs.is_symlink(),
            size: attrs.len(),
            modified,
            permissions: attrs.permissions,
        }
    }
}

/// Manages SFTP sessions for multiple hosts.
///
/// The session map is locked only to look an entry up, insert or remove it,
/// never across network I/O. A transfer or a listing therefore holds up
/// nothing but itself: other sessions keep working, and `close` and
/// `close_all` can always take an entry out of reach at once.
pub struct SftpManager {
    sessions: Mutex<HashMap<String, Arc<SftpConnection>>>,
    /// The local files a download is under way to, across every session: the
    /// local disk is one for all of them. See [`download_destination`].
    downloads: WriterLocks<PathBuf>,
}

struct SftpConnection {
    handle: Handle<connection::SshHandler>,
    transport: Transport,
    sftp: SftpSession,
    /// A second SFTP channel that serves directory listings, request by
    /// request. `SftpSession::read_dir` collects a whole directory before it
    /// returns, so it can neither cap a listing's size nor close the
    /// directory handle when a listing is abandoned. `None` when the server
    /// would not open the channel; listings then fall back to `read_dir`
    /// under [`FALLBACK_LIST_DEADLINE`].
    lister: Option<RawSftpSession>,
    /// Raised once the connection is closed. Every operation watches it and
    /// stops, so one that is stuck behind a peer that stopped reading, or
    /// simply still running, cannot outlive the close or report success
    /// after it.
    closed: CloseSignal,
    /// The remote paths an upload or write is under way to, each with the
    /// lock the next writer to that path waits on; see
    /// [`SftpConnection::claim_destination`].
    writers: WriterLocks<String>,
}

/// One lock per destination a writer is under way to, the next writer to the
/// same destination waiting on it.
type WriterLocks<K> = std::sync::Mutex<HashMap<K, Arc<Mutex<()>>>>;

fn lock_writers<K>(
    writers: &WriterLocks<K>,
) -> std::sync::MutexGuard<'_, HashMap<K, Arc<Mutex<()>>>> {
    // The map only ever holds valid locks, whatever a panicking holder was doing.
    writers.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sole right to write one destination, released on drop.
struct DestinationGuard<'a, K: Eq + Hash> {
    writers: &'a WriterLocks<K>,
    destination: K,
    lock: Option<Arc<Mutex<()>>>,
    held: Option<OwnedMutexGuard<()>>,
}

impl<K: Eq + Hash> Drop for DestinationGuard<'_, K> {
    fn drop(&mut self) {
        drop(self.held.take());
        let mut writers = lock_writers(self.writers);
        drop(self.lock.take());
        // Every writer holding or waiting on the destination keeps a clone, so
        // a count of one means the map's own copy is the last and the entry
        // can go. The count is read under the map lock, which every clone is
        // taken under, so no writer can be joining at the same moment.
        if writers
            .get(&self.destination)
            .is_some_and(|lock| Arc::strong_count(lock) == 1)
        {
            writers.remove(&self.destination);
        }
    }
}

/// Wait until no other writer in `writers` is writing `destination`, then hold
/// it until the guard drops.
async fn claim<K: Eq + Hash + Clone>(
    writers: &WriterLocks<K>,
    destination: K,
) -> DestinationGuard<'_, K> {
    let lock = lock_writers(writers)
        .entry(destination.clone())
        .or_default()
        .clone();
    // Built before the wait so that a writer cancelled while waiting still
    // takes its entry out of the map when nothing else needs it.
    let mut guard = DestinationGuard {
        writers,
        destination,
        lock: Some(lock.clone()),
        held: None,
    };
    guard.held = Some(lock.lock_owned().await);
    guard
}

/// The key a download to `local_path` is serialized under: the path with its
/// directory resolved, so that two spellings of one directory, through a
/// symlink or a `..`, still name one file. Falls back to the path as given
/// when the directory cannot be resolved; the open that follows then fails
/// on its own.
async fn download_destination(local_path: &Path) -> PathBuf {
    let resolved = match (local_path.parent(), local_path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            tokio::fs::canonicalize(parent)
                .await
                .ok()
                .map(|parent| parent.join(name))
        }
        _ => None,
    };
    resolved.unwrap_or_else(|| local_path.to_path_buf())
}

impl SftpConnection {
    /// Wait until no other upload or write on this connection is writing
    /// `path`, then hold it until the guard drops. SFTP calls on one connection
    /// run side by side, and two writers on one file would otherwise truncate
    /// and write it at once, leaving a mix of both that each reports as a
    /// success. Paths are compared as given, which is how the browser builds
    /// them.
    async fn claim_destination(&self, path: &str) -> DestinationGuard<'_, String> {
        claim(&self.writers, path.to_owned()).await
    }

    /// Run `operation` until it finishes or the connection is closed.
    async fn until_closed<T>(&self, operation: impl Future<Output = Result<T>>) -> Result<T> {
        self.closed
            .guard(operation, || {
                CoreError::Ssh("the SFTP session was closed".into())
            })
            .await
    }

    /// Stop every request, listing and transfer on this connection, end its
    /// SFTP channels, then the connection. The goodbye is bounded and cuts the
    /// connection if the peer does not take it, so an unresponsive peer can
    /// neither hold the caller nor keep the connection open.
    async fn close(&self) {
        self.closed.raise();
        let _ = self.sftp.close().await;
        if let Some(lister) = &self.lister {
            let _ = lister.close_session();
        }
        transport::goodbye(&self.handle, &self.transport).await;
    }
}

/// Open a channel on `handle` and start the SFTP subsystem on it.
async fn open_sftp_channel(
    handle: &Handle<connection::SshHandler>,
) -> Result<russh::ChannelStream<russh::client::Msg>> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| CoreError::Ssh(format!("sftp channel open: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| CoreError::Ssh(format!("request sftp subsystem: {e}")))?;
    Ok(channel.into_stream())
}

/// Open the listing channel, or `None` when the server will not have it, for
/// instance one that allows a single session per connection.
async fn open_lister(handle: &Handle<connection::SshHandler>) -> Option<RawSftpSession> {
    let opened = async {
        let lister = RawSftpSession::new(open_sftp_channel(handle).await.ok()?);
        lister.init().await.ok()?;
        Some(lister)
    };
    tokio::time::timeout(LISTER_OPEN_TIMEOUT, opened)
        .await
        .ok()
        .flatten()
}

/// Read the directory behind `handle` page by page, refusing it once it
/// passes [`MAX_LIST_ENTRIES`] or [`MAX_LIST_NAME_BYTES`]. Each page is bounded
/// by the session's per-request timeout, so a server that goes quiet ends the
/// listing too.
async fn read_listing(lister: &RawSftpSession, handle: &str) -> Result<Vec<SftpEntry>> {
    let too_large = || {
        CoreError::Ssh(format!(
            "read_dir: the directory has more than {MAX_LIST_ENTRIES} entries or \
             {} MiB of names, more than Clavyn lists",
            MAX_LIST_NAME_BYTES / (1024 * 1024)
        ))
    };
    let mut entries = Vec::new();
    let mut name_bytes = 0usize;
    loop {
        match lister.readdir(handle).await {
            Ok(page) => {
                for file in page.files {
                    if file.filename == "." || file.filename == ".." {
                        continue;
                    }
                    name_bytes = name_bytes.saturating_add(file.filename.len());
                    if entries.len() >= MAX_LIST_ENTRIES || name_bytes > MAX_LIST_NAME_BYTES {
                        return Err(too_large());
                    }
                    entries.push(SftpEntry::new(
                        file.filename.clone(),
                        file.filename,
                        &file.attrs,
                    ));
                }
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                return Ok(entries);
            }
            Err(e) => return Err(CoreError::Ssh(format!("read_dir: {e}"))),
        }
    }
}

impl SftpManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            downloads: WriterLocks::default(),
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

        let connection::Connection { handle, transport } =
            connection::connect(host, identity, known_hosts, vault, passphrase, password).await?;

        // The two channels open side by side so the listing channel adds no
        // round trips to the connect.
        let session = async {
            SftpSession::new(open_sftp_channel(&handle).await?)
                .await
                .map_err(|e| CoreError::Ssh(format!("sftp init: {e}")))
        };
        let (sftp, lister) = tokio::join!(session, open_lister(&handle));
        let sftp = match sftp {
            Ok(sftp) => sftp,
            Err(e) => {
                transport.cut();
                return Err(e);
            }
        };

        let conn = Arc::new(SftpConnection {
            handle,
            transport,
            sftp,
            lister,
            closed: CloseSignal::new(),
            writers: WriterLocks::default(),
        });
        // The check at the top ran before a connect that takes seconds, so a
        // second connect on the same id may have finished in the meantime. The
        // session already published keeps the id; this one is ended rather
        // than silently replacing it.
        match self.sessions.lock().await.entry(session_id) {
            std::collections::hash_map::Entry::Occupied(_) => {
                conn.transport.cut();
                Err(CoreError::InvalidInput(
                    "session id is already in use by an SFTP session".into(),
                ))
            }
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

    /// Run one request on `session_id`'s connection until it answers or the
    /// connection is closed, and name the request in its error.
    async fn request<T, F>(
        &self,
        session_id: &str,
        what: &str,
        request: impl FnOnce(Arc<SftpConnection>) -> F,
    ) -> Result<T>
    where
        F: Future<Output = std::result::Result<T, SftpError>>,
    {
        let conn = self.get(session_id).await?;
        let pending = request(conn.clone());
        conn.until_closed(async move {
            pending
                .await
                .map_err(|e| CoreError::Ssh(format!("{what}: {e}")))
        })
        .await
    }

    /// List directory entries at the given path.
    pub async fn list_dir(&self, session_id: &str, path: &str) -> Result<Vec<SftpEntry>> {
        let conn = self.get(session_id).await?;

        let mut entries = conn
            .until_closed(async {
                match &conn.lister {
                    Some(lister) => {
                        let handle = lister
                            .opendir(path)
                            .await
                            .map_err(|e| CoreError::Ssh(format!("read_dir: {e}")))?
                            .handle;
                        let listed = read_listing(lister, &handle).await;
                        // Closed however the listing went, so one that failed
                        // or was refused leaves no handle open on the server.
                        let _ = lister.close(handle).await;
                        listed
                    }
                    None => {
                        let dir = tokio::time::timeout(
                            FALLBACK_LIST_DEADLINE,
                            conn.sftp.read_dir(path),
                        )
                        .await
                        .map_err(|_| {
                            CoreError::Ssh(format!(
                                "read_dir: the listing took longer than {}s",
                                FALLBACK_LIST_DEADLINE.as_secs()
                            ))
                        })?
                        .map_err(|e| CoreError::Ssh(format!("read_dir: {e}")))?;
                        Ok(dir
                            .map(|entry| {
                                let name = entry.file_name();
                                SftpEntry::new(name.clone(), name, &entry.metadata())
                            })
                            .collect())
                    }
                }
            })
            .await?;

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
        self.request(session_id, "canonicalize", |conn| async move {
            conn.sftp.canonicalize(path).await
        })
        .await
    }

    /// Read a file's contents into a byte vector.
    ///
    /// Kept for API compatibility. Interactive file transfers should use
    /// `download_to_local` so large files never cross Tauri IPC as JSON arrays.
    pub async fn read_file(&self, session_id: &str, path: &str) -> Result<Vec<u8>> {
        self.request(session_id, "read file", |conn| async move {
            conn.sftp.read(path).await
        })
        .await
    }

    /// Write data to a remote file (creates or overwrites).
    ///
    /// Kept for API compatibility. Interactive file transfers should use
    /// `upload_from_local` so large files never cross Tauri IPC as JSON arrays.
    pub async fn write_file(&self, session_id: &str, path: &str, data: &[u8]) -> Result<()> {
        self.request(session_id, "write file", |conn| async move {
            let _destination = conn.claim_destination(path).await;
            conn.sftp.write(path, data).await
        })
        .await
    }

    /// Stream a remote file directly to a local path. An existing local file is
    /// refused unless the caller explicitly opts in to overwrite, mirroring
    /// `upload_from_local`. A download waits for any other download to the same
    /// local file, from any session, to finish first.
    pub async fn download_to_local(
        &self,
        session_id: &str,
        remote_path: &str,
        local_path: &Path,
        overwrite: bool,
    ) -> Result<()> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            // Two downloads to one local file would otherwise truncate and
            // write it at once, from any two sessions, leaving a mix of both
            // that each reports as a success.
            let destination = download_destination(local_path).await;
            let _destination = claim(&self.downloads, destination).await;
            let mut remote = conn
                .sftp
                .open(remote_path)
                .await
                .map_err(|e| CoreError::Ssh(format!("open remote file: {e}")))?;
            let mut local = open_download_target(local_path, overwrite).await?;

            tokio::io::copy(&mut remote, &mut local).await?;
            local.flush().await?;
            Ok(())
        })
        .await
    }

    /// Stream a local file directly to the remote host. Existing remote files
    /// are rejected unless the caller explicitly opts in to overwrite. An
    /// upload waits for any other upload to the same path on this connection
    /// to finish first.
    pub async fn upload_from_local(
        &self,
        session_id: &str,
        local_path: &Path,
        remote_path: &str,
        overwrite: bool,
    ) -> Result<()> {
        let conn = self.get(session_id).await?;

        conn.until_closed(async {
            let _destination = conn.claim_destination(remote_path).await;
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
            // Without `overwrite` the create is exclusive, so of two uploads to
            // one path the second is refused even when both passed the check
            // above before either had created the file.
            let created = if overwrite {
                conn.sftp.create(remote_path).await
            } else {
                conn.sftp
                    .open_with_flags(
                        remote_path,
                        OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                    )
                    .await
            };
            let mut remote = match created {
                Ok(remote) => remote,
                Err(_) if !overwrite && conn.sftp.try_exists(remote_path).await.unwrap_or(false) => {
                    return Err(CoreError::InvalidInput(format!(
                        "remote file already exists: {remote_path}"
                    )));
                }
                Err(e) => return Err(CoreError::Ssh(format!("create remote file: {e}"))),
            };

            tokio::io::copy(&mut local, &mut remote).await?;
            remote.flush().await?;
            remote.shutdown().await?;
            Ok(())
        })
        .await
    }

    /// Create a directory.
    pub async fn create_dir(&self, session_id: &str, path: &str) -> Result<()> {
        self.request(session_id, "mkdir", |conn| async move {
            conn.sftp.create_dir(path).await
        })
        .await
    }

    /// Remove a file.
    pub async fn remove_file(&self, session_id: &str, path: &str) -> Result<()> {
        self.request(session_id, "remove file", |conn| async move {
            conn.sftp.remove_file(path).await
        })
        .await
    }

    /// Remove a directory.
    pub async fn remove_dir(&self, session_id: &str, path: &str) -> Result<()> {
        self.request(session_id, "remove dir", |conn| async move {
            conn.sftp.remove_dir(path).await
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
        self.request(session_id, "rename", |conn| async move {
            conn.sftp.rename(old_path, new_path).await
        })
        .await
    }

    /// Get file metadata (size, permissions, modified time).
    pub async fn stat(&self, session_id: &str, path: &str) -> Result<SftpEntry> {
        let attrs = self
            .request(session_id, "stat", |conn| async move {
                conn.sftp.metadata(path).await
            })
            .await?;
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        Ok(SftpEntry::new(name, path.to_string(), &attrs))
    }

    /// Close an SFTP session. The entry leaves the map before anything is
    /// sent, and closing the connection ends any transfer still running on it.
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
    /// listings never hold the lock, so the drain does not wait on them, and
    /// every connection is marked closed before any goodbye is sent.
    pub async fn close_all(&self) -> usize {
        let drained: Vec<Arc<SftpConnection>> = {
            let mut sessions = self.sessions.lock().await;
            sessions.drain().map(|(_, conn)| conn).collect()
        };
        let count = drained.len();
        let goodbyes: Vec<_> = drained
            .into_iter()
            .map(|conn| {
                conn.closed.raise();
                tokio::spawn(async move { conn.close().await })
            })
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
    use super::open_download_target;
    use tokio::io::AsyncWriteExt;

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
