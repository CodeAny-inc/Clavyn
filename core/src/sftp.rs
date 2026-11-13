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
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWriteExt};
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

/// Buffer between a transfer and its local file.
///
/// tokio runs every read or write of a `tokio::fs::File` as a separate job on
/// its blocking pool, so this sets how much each of those jobs moves. On an
/// upload it also keeps every SFTP write full: russh-sftp splits writes into
/// packets of at most `Config::max_write_packet_len` (32 KiB by default) and
/// keeps up to 16 in flight, but sends a smaller packet whenever it is handed
/// less, as it is by `tokio::io::copy`'s 8 KiB reads. With full packets an
/// upload keeps about 512 KiB in flight instead of 128 KiB.
const LOCAL_FILE_BUFFER: usize = 256 * 1024;

/// Most symbolic links a download follows to find the file it writes, the
/// same bound Linux puts on a path lookup.
const MAX_LINKS: usize = 40;

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
        self.closed.guard(operation, session_closed).await
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
    /// local file, from any session, to finish first, so of two downloads to
    /// one file the one started last is the one left in place.
    pub async fn download_to_local(
        &self,
        session_id: &str,
        remote_path: &str,
        local_path: &Path,
        overwrite: bool,
    ) -> Result<()> {
        let conn = self.get(session_id).await?;

        let (_destination, mut remote) = conn
            .until_closed(async {
                let destination = download_destination(local_path).await;
                let destination = claim(&self.downloads, destination).await;
                let remote = conn
                    .sftp
                    .open(remote_path)
                    .await
                    .map_err(|e| CoreError::Ssh(format!("open remote file: {e}")))?;
                Ok((destination, remote))
            })
            .await?;
        // A close stops the download at any point before it is put in place
        // but never cuts its cleanup short, so the staging file is always
        // removed by the download itself.
        write_download(&mut remote, local_path, overwrite, conn.closed.raised()).await
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

            let mut local = tokio::io::BufReader::with_capacity(
                LOCAL_FILE_BUFFER,
                tokio::fs::File::open(local_path).await?,
            );
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

            tokio::io::copy_buf(&mut local, &mut remote).await?;
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

/// The error every operation stopped by a close reports.
fn session_closed() -> CoreError {
    CoreError::Ssh("the SFTP session was closed".into())
}

/// Write a download to `target` without ever leaving a partial file under its
/// name.
///
/// Every download is written to a staging file beside the target and put in
/// place only once every byte is on disk, so a dropped connection, a server
/// timeout, a closed session or the app quitting mid-transfer never leaves a
/// file under the target's name that looks finished. A failed download removes
/// only its own staging file, so it never takes a file that another download
/// put at the same name.
///
/// With `overwrite`, whatever is at `target` is replaced, provided it is a
/// regular file the user could also have written in place: one the user has
/// no write access to, such as a file marked read-only, is refused rather than
/// renamed over.
/// The replacement keeps the replaced file's permission bits. It is still a
/// new file, though, so other hard links to the old one keep the old content,
/// its owner, extended attributes and Windows access control are its own, and
/// replacing needs room for both copies until the rename. Without `overwrite`,
/// an existing file is refused, and the download takes the name only if
/// nothing else has taken it in the meantime.
///
/// A symbolic link is followed, even one whose file does not exist yet, so the
/// file it names is written and the link stays a link, as writing through it
/// would have done. `closed` resolving stops the download at any point before
/// it is put in place; the cleanup still runs.
async fn write_download<R: AsyncRead + Unpin + ?Sized>(
    source: &mut R,
    target: &Path,
    overwrite: bool,
    closed: impl Future<Output = ()>,
) -> Result<()> {
    let mut closed = std::pin::pin!(closed);
    if !overwrite && tokio::fs::symlink_metadata(target).await.is_ok() {
        return Err(already_exists(target));
    }
    let target = follow_link(target).await;
    let replaced = if overwrite {
        replaceable(&target).await?
    } else {
        None
    };

    let staging = staging_path(&target);
    let mut file = open_staging(&staging, replaced.is_some()).await?;
    let copied = copy_until(source, &mut file, closed.as_mut()).await;
    if let (Ok(()), Some(replaced)) = (&copied, &replaced) {
        keep_permissions(&file, replaced).await;
    }
    close(file).await;

    let published = match copied {
        Err(error) => Err(error),
        // A close that lands while the file is being made durable still stops
        // the download, so nothing is put in place once the session is gone.
        Ok(()) if has_resolved(closed.as_mut()).await => Err(session_closed()),
        Ok(()) => publish(&staging, &target, overwrite).await,
    };
    if published.is_err() {
        discard(&staging).await;
        return published;
    }
    sync_directory_of(&target).await;
    Ok(())
}

/// Copy `source` into `file` and make it durable, unless `closed` resolves
/// first.
async fn copy_until<R, F>(
    source: &mut R,
    file: &mut tokio::fs::File,
    closed: Pin<&mut F>,
) -> Result<()>
where
    R: AsyncRead + Unpin + ?Sized,
    F: Future<Output = ()>,
{
    let mut writer = tokio::io::BufWriter::with_capacity(LOCAL_FILE_BUFFER, &mut *file);
    tokio::select! {
        copied = tokio::io::copy(source, &mut writer) => {
            copied?;
        }
        () = closed => return Err(session_closed()),
    }
    writer.flush().await?;
    drop(writer);
    file.sync_all().await?;
    Ok(())
}

/// Whether `future` has resolved, without waiting for it. Never called on a
/// future that has already resolved.
async fn has_resolved<F: Future<Output = ()>>(mut future: Pin<&mut F>) -> bool {
    std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_ready())).await
}

/// Close `file` once tokio has finished any write still in flight on it.
/// Dropping a `tokio::fs::File` leaves the handle open until then, and the
/// file is renamed or removed right after this.
async fn close(mut file: tokio::fs::File) {
    let _ = file.flush().await;
    drop(file.try_into_std());
}

/// The path a write through `path` ends up at. A symbolic link is followed
/// even when the file it names does not exist yet, since writing through it
/// would create that file. A chain longer than `MAX_LINKS` is left as it is,
/// so opening it fails the way the system reports a loop.
async fn follow_link(path: &Path) -> PathBuf {
    let mut path = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        let is_link = tokio::fs::symlink_metadata(&path)
            .await
            .is_ok_and(|meta| meta.file_type().is_symlink());
        if !is_link {
            break;
        }
        let Ok(next) = tokio::fs::read_link(&path).await else {
            break;
        };
        // A relative link names a path from the directory holding the link;
        // `join` keeps an absolute one as it is.
        path = match path.parent() {
            Some(dir) => dir.join(next),
            None => next,
        };
    }
    path
}

/// The file an overwrite of `target` replaces, if there is one.
///
/// Writing a file in place needs permission to write that file, while renaming
/// over it needs permission only on its directory. The download is put in
/// place by a rename, so the file is first opened for writing, and one the user
/// could not have written is refused rather than replaced. Only a regular file
/// is replaced.
async fn replaceable(target: &Path) -> Result<Option<std::fs::Metadata>> {
    let replaced = match tokio::fs::metadata(target).await {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !replaced.is_file() {
        return Err(CoreError::InvalidInput(format!(
            "not a regular file: {}",
            target.display()
        )));
    }
    tokio::fs::OpenOptions::new()
        .write(true)
        .open(target)
        .await
        .map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot write {}: {error}", target.display()),
            )
        })?;
    Ok(Some(replaced))
}

fn already_exists(path: &Path) -> CoreError {
    CoreError::InvalidInput(format!("local file already exists: {}", path.display()))
}

/// A fresh hidden name beside `target`. Only the start of the target's name
/// is kept, so a name close to the file system's length limit still leaves
/// room for the suffix.
fn staging_path(target: &Path) -> PathBuf {
    let name: String = target
        .file_name()
        .map(|n| n.to_string_lossy().chars().take(32).collect())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.{}.part", uuid::Uuid::new_v4()))
}

/// Create the staging file. One that replaces an existing file starts out
/// readable by its owner only on Unix, so a private file being replaced is
/// never readable by anyone else mid-download; a new download starts with the
/// default permissions it keeps. On Windows both take the access control the
/// directory passes on.
async fn open_staging(path: &Path, private: bool) -> Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    Ok(options.open(path).await?)
}

/// Give a replacement the permission bits of the file it replaces. Set-user-ID
/// and set-group-ID are left off, as a write in place by anyone but root clears
/// them. A failure is logged and the staging file stays owner-only, which is
/// never more open than the original: some network and FUSE file systems
/// refuse every mode change, and a complete download is not thrown away for it.
#[cfg(unix)]
async fn keep_permissions(file: &tokio::fs::File, replaced: &std::fs::Metadata) {
    use std::os::unix::fs::PermissionsExt;
    let mode = replaced.permissions().mode() & 0o777;
    if let Err(error) = file
        .set_permissions(std::fs::Permissions::from_mode(mode))
        .await
    {
        tracing::warn!("could not give a download the replaced file's permissions: {error}");
    }
}

/// Windows has no permission bits to carry over, and a file that is read-only
/// is refused before the download starts.
#[cfg(not(unix))]
async fn keep_permissions(_file: &tokio::fs::File, _replaced: &std::fs::Metadata) {}

/// Put a finished staging file in place at `target`.
///
/// With `replace`, whatever is at `target` is replaced. Without it, the file
/// takes the name only if nothing else has: a hard link refuses an existing
/// name in the same step that creates it. A file system without hard links
/// (FAT, exFAT, some network shares) gets a check followed by a rename
/// instead, which leaves a short window in which a file created by someone
/// else could be replaced.
async fn publish(staging: &Path, target: &Path, replace: bool) -> Result<()> {
    if replace {
        return Ok(tokio::fs::rename(staging, target).await?);
    }
    match tokio::fs::hard_link(staging, target).await {
        Ok(()) => {
            discard(staging).await;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(already_exists(target))
        }
        Err(_) => {
            if tokio::fs::symlink_metadata(target).await.is_ok() {
                return Err(already_exists(target));
            }
            Ok(tokio::fs::rename(staging, target).await?)
        }
    }
}

/// Make the download's name durable as well as its bytes. A failure is
/// logged: the download is already in place, and reporting it as failed would
/// say the opposite.
async fn sync_directory_of(target: &Path) {
    let target = target.to_path_buf();
    let synced = tokio::task::spawn_blocking(move || {
        crate::fs_util::sync_parent_directory(&target).map_err(|error| (target, error))
    })
    .await;
    if let Ok(Err((target, error))) = synced {
        tracing::warn!("saved {} but could not sync its directory: {error}", target.display());
    }
}

/// Remove what a failed download wrote. A file that cannot be removed is
/// logged, since the error the caller sees is the download's own.
async fn discard(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("could not remove an incomplete download at {}: {error}", path.display());
        }
    }
}

#[cfg(test)]
mod download_target_tests {
    use super::write_download;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, ReadBuf};

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

        write_download(&mut &b"replacement"[..], &path, true, never())
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

        assert!(write_download(&mut BrokenAfter(b"half of the"), &path, true, never())
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }

    /// Fails after another download has renamed its finished file over the
    /// path this one was writing.
    struct ReplacedThenBroken(PathBuf);

    impl AsyncRead for ReplacedThenBroken {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let other = self.0.with_extension("other");
            std::fs::write(&other, "finished by another download").expect("write");
            std::fs::rename(&other, &self.0).expect("rename");
            Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()))
        }
    }

    #[tokio::test]
    async fn a_failed_new_download_spares_a_file_that_replaced_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");

        assert!(
            write_download(&mut ReplacedThenBroken(path.clone()), &path, false, never())
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("the other download's file"),
            "finished by another download"
        );
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }

    #[tokio::test]
    async fn a_failed_new_download_leaves_nothing_behind() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");

        assert!(write_download(&mut BrokenAfter(b"half of the"), &path, false, never())
            .await
            .is_err());
        assert!(entries(dir.path()).is_empty());
    }

    /// Yields `data` and notes whether anything was at `path` while it did.
    struct WatchesName {
        path: PathBuf,
        data: &'static [u8],
        saw_the_name: bool,
    }

    impl AsyncRead for WatchesName {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let this = &mut *self;
            this.saw_the_name |= std::fs::symlink_metadata(&this.path).is_ok();
            let n = this.data.len().min(buf.remaining());
            buf.put_slice(&this.data[..n]);
            this.data = &this.data[n..];
            Poll::Ready(Ok(()))
        }
    }

    /// A new download is staged too, so its name never holds a partial file:
    /// not while it runs, and not if the app quits part way through.
    #[tokio::test]
    async fn a_new_download_takes_its_name_only_once_complete() {
        let dir = tempfile::tempdir().expect("temp dir");
        for (name, overwrite) in [("a.txt", false), ("b.txt", true)] {
            let path = dir.path().join(name);
            let mut source = WatchesName {
                path: path.clone(),
                data: b"complete",
                saw_the_name: false,
            };
            write_download(&mut source, &path, overwrite, never())
                .await
                .expect("download");
            assert!(!source.saw_the_name, "{name} existed mid-download");
            assert_eq!(std::fs::read_to_string(&path).expect("read"), "complete");
        }
        assert_eq!(entries(dir.path()), ["a.txt", "b.txt"]);
    }

    /// Writes another file at `path` as the download starts, then yields
    /// `data`.
    struct NameTakenDuring {
        path: Option<PathBuf>,
        data: &'static [u8],
    }

    impl AsyncRead for NameTakenDuring {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some(path) = self.path.take() {
                std::fs::write(path, "finished by another download").expect("write");
            }
            let n = self.data.len().min(buf.remaining());
            buf.put_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_download_without_overwrite_never_replaces_a_file_that_took_its_name() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        let mut source = NameTakenDuring {
            path: Some(path.clone()),
            data: b"this download",
        };

        assert!(write_download(&mut source, &path, false, never())
            .await
            .is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "finished by another download"
        );
        assert_eq!(entries(dir.path()), ["report.txt"]);
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

    fn never() -> std::future::Pending<()> {
        std::future::pending()
    }

    fn soon() -> tokio::time::Sleep {
        tokio::time::sleep(std::time::Duration::from_millis(200))
    }

    #[tokio::test]
    async fn a_download_stopped_by_a_close_leaves_the_original_and_nothing_else() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");

        let stopped = write_download(&mut StallsAfter(b"half of the"), &path, true, soon()).await;
        assert!(stopped.is_err(), "the stalled download finished");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");
        assert_eq!(entries(dir.path()), ["report.txt"]);

        let fresh = dir.path().join("new.txt");
        assert!(write_download(&mut StallsAfter(b"half of the"), &fresh, true, soon())
            .await
            .is_err());
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }

    /// Yields `data`, then reports the session closed as the data runs out.
    struct ClosesAtTheEnd {
        data: &'static [u8],
        closed: Arc<AtomicBool>,
    }

    impl AsyncRead for ClosesAtTheEnd {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.data.is_empty() {
                self.closed.store(true, Ordering::SeqCst);
                return Poll::Ready(Ok(()));
            }
            let n = self.data.len().min(buf.remaining());
            buf.put_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Poll::Ready(Ok(()))
        }
    }

    /// A close that arrives once every byte has been read still stops the
    /// download, wherever it is in making the file durable.
    #[tokio::test]
    async fn a_close_after_the_last_byte_still_leaves_the_original() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");
        let closed = Arc::new(AtomicBool::new(false));
        let mut source = ClosesAtTheEnd {
            data: b"replacement",
            closed: closed.clone(),
        };
        let signal = std::future::poll_fn(move |_| {
            if closed.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        });

        assert!(write_download(&mut source, &path, true, signal).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }

    /// The replacement takes the original's permissions, so a private file
    /// stays private.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_overwrite_keeps_the_original_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("id_backup");
        std::fs::write(&path, "original").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");

        write_download(&mut &b"replacement"[..], &path, true, never())
            .await
            .expect("download");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    /// A write in place clears set-user-ID and set-group-ID, so a replacement
    /// does not carry them over.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_overwrite_does_not_carry_set_id_bits_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("tool");
        std::fs::write(&path, "original").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o6755)).expect("chmod");

        write_download(&mut &b"replacement"[..], &path, true, never())
            .await
            .expect("download");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o7777;
        assert_eq!(mode, 0o755);
    }

    /// A new download ends up with the permissions any new file in that
    /// directory gets.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_download_gets_the_default_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temp dir");
        let reference = dir.path().join("reference");
        std::fs::write(&reference, "").expect("write");
        let path = dir.path().join("report.txt");

        write_download(&mut &b"data"[..], &path, true, never())
            .await
            .expect("download");
        let mode = |path: &std::path::Path| {
            std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
        };
        assert_eq!(mode(&path), mode(&reference));
    }

    /// A file the user could not have written in place is refused. A rename
    /// needs write access only to the directory, so it must not stand in for
    /// a write the user has no permission for.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_overwrite_refuses_a_file_the_user_cannot_write() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).expect("chmod");
        // An account that may write any file, root for one, has nothing to be
        // refused here.
        if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
            return;
        }

        assert!(write_download(&mut &b"replacement"[..], &path, true, never())
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }

    /// Overwriting through a symbolic link replaces the file it names and
    /// leaves the link in place.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_overwrite_through_a_link_replaces_the_file_it_names() {
        let dir = tempfile::tempdir().expect("temp dir");
        let real = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&real, "original").expect("write");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        write_download(&mut &b"replacement"[..], &link, true, never())
            .await
            .expect("download");
        assert!(std::fs::symlink_metadata(&link).expect("lstat").file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).expect("read"), "replacement");
        assert_eq!(entries(dir.path()), ["link.txt", "real.txt"]);
    }

    /// Writing through a link whose file does not exist yet creates that file,
    /// as writing through it in place would.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_overwrite_through_a_dangling_link_creates_the_file_it_names() {
        let dir = tempfile::tempdir().expect("temp dir");
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink("real.txt", &link).expect("symlink");

        write_download(&mut &b"replacement"[..], &link, true, never())
            .await
            .expect("download");
        assert!(std::fs::symlink_metadata(&link).expect("lstat").file_type().is_symlink());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("real.txt")).expect("read"),
            "replacement"
        );
        assert_eq!(entries(dir.path()), ["link.txt", "real.txt"]);
    }

    #[tokio::test]
    async fn an_overwrite_works_for_a_name_at_the_length_limit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(format!("{}.txt", "n".repeat(251)));
        std::fs::write(&path, "original").expect("write");

        write_download(&mut &b"replacement"[..], &path, true, never())
            .await
            .expect("download");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "replacement");
    }

    #[tokio::test]
    async fn a_download_without_overwrite_keeps_an_existing_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "original").expect("write");

        assert!(write_download(&mut &b"replacement"[..], &path, false, never())
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original");
        assert_eq!(entries(dir.path()), ["report.txt"]);
    }
}
