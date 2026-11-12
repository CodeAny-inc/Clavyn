use crate::connection;
use crate::host::Host;
use crate::identity::Identity;
use crate::known_hosts::KnownHosts;
use crate::vault::Vault;
use crate::{CoreError, Result};
use russh::client::Handle;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
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
            _ = closed.wait_for(|closed| *closed) => Err(session_closed()),
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

        let mut remote = conn
            .until_closed(async {
                conn.sftp
                    .open(remote_path)
                    .await
                    .map_err(|e| CoreError::Ssh(format!("open remote file: {e}")))
            })
            .await?;
        // A close stops the download at any point before it is put in place
        // but never cuts its cleanup short, so the staging file is always
        // removed by the download itself.
        let mut closed = conn.closed.subscribe();
        let closed = async move {
            let _ = closed.wait_for(|closed| *closed).await;
        };
        write_download(&mut remote, local_path, overwrite, closed).await
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
                LOCAL_FILE_BUFFER,
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
