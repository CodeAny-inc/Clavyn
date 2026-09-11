use crate::state::{AppState, LocalTerminal, LocalTerminals};
use clavyn_core::host::{AuthMethod, Host, HostGroup};
use clavyn_core::identity::Identity;
use clavyn_core::keys::{generate_ed25519, parse_openssh_private, KeyMeta};
use clavyn_core::sftp::SftpEntry;
use clavyn_core::workspace::Workspace;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

type ApiResult<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ============================================================
// Hosts
// ============================================================

#[tauri::command]
pub async fn list_hosts(state: State<'_, Arc<AppState>>) -> ApiResult<Vec<Host>> {
    let store = state.store.lock().await;
    Ok(store.hosts().to_vec())
}

#[tauri::command]
pub async fn add_host(
    state: State<'_, Arc<AppState>>,
    host: Host,
) -> ApiResult<Host> {
    let mut store = state.store.lock().await;
    store.add_host(host.clone()).map_err(err)?;
    Ok(host)
}

#[tauri::command]
pub async fn update_host(
    state: State<'_, Arc<AppState>>,
    host: Host,
) -> ApiResult<Host> {
    let mut store = state.store.lock().await;
    store.update_host(host.clone()).map_err(err)?;
    Ok(host)
}

#[tauri::command]
pub async fn delete_host(
    state: State<'_, Arc<AppState>>,
    id: Uuid,
) -> ApiResult<()> {
    let mut store = state.store.lock().await;
    store.remove_host(id).map_err(err)
}

// ============================================================
// Host Groups
// ============================================================

#[tauri::command]
pub async fn list_groups(state: State<'_, Arc<AppState>>) -> ApiResult<Vec<HostGroup>> {
    let store = state.store.lock().await;
    Ok(store.groups().to_vec())
}

#[tauri::command]
pub async fn add_group(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> ApiResult<HostGroup> {
    let group = HostGroup::new(name);
    let mut store = state.store.lock().await;
    store.add_group(group.clone()).map_err(err)?;
    Ok(group)
}

#[tauri::command]
pub async fn delete_group(
    state: State<'_, Arc<AppState>>,
    id: Uuid,
) -> ApiResult<()> {
    let mut store = state.store.lock().await;
    store.remove_group(id).map_err(err)
}

// ============================================================
// Identities
// ============================================================

#[tauri::command]
pub async fn list_identities(state: State<'_, Arc<AppState>>) -> ApiResult<Vec<Identity>> {
    let store = state.store.lock().await;
    Ok(store.identities().to_vec())
}

#[tauri::command]
pub async fn add_identity(
    state: State<'_, Arc<AppState>>,
    identity: Identity,
) -> ApiResult<Identity> {
    let mut store = state.store.lock().await;
    store.add_identity(identity.clone()).map_err(err)?;
    Ok(identity)
}

#[tauri::command]
pub async fn update_identity(
    state: State<'_, Arc<AppState>>,
    identity: Identity,
) -> ApiResult<Identity> {
    let mut store = state.store.lock().await;
    store.update_identity(identity.clone()).map_err(err)?;
    Ok(identity)
}

#[tauri::command]
pub async fn delete_identity(
    state: State<'_, Arc<AppState>>,
    id: Uuid,
) -> ApiResult<()> {
    let mut store = state.store.lock().await;
    store.remove_identity(id).map_err(err)
}

// ============================================================
// Vault
// ============================================================

#[tauri::command]
pub async fn vault_is_initialized(state: State<'_, Arc<AppState>>) -> ApiResult<bool> {
    let vault = state.vault.lock().await;
    Ok(vault.is_initialized())
}

#[tauri::command]
pub async fn is_vault_unlocked(state: State<'_, Arc<AppState>>) -> ApiResult<bool> {
    let pw = state.passphrase.lock().await;
    Ok(pw.is_some())
}

// ============================================================
// Keys
// ============================================================

#[tauri::command]
pub async fn list_keys(state: State<'_, Arc<AppState>>) -> ApiResult<Vec<KeyMeta>> {
    let vault = state.vault.lock().await;
    Ok(vault.keys_meta().to_vec())
}

#[tauri::command]
pub async fn generate_key(
    state: State<'_, Arc<AppState>>,
    label: String,
) -> ApiResult<KeyMeta> {
    let (private, _public) = generate_ed25519().map_err(err)?;
    let (mut meta, _pair) = parse_openssh_private(&private, None).map_err(err)?;
    meta.label = label;
    let passphrase = {
        let pw = state.passphrase.lock().await;
        pw.as_ref()
            .map(|p| p.to_string())
            .ok_or_else(|| "vault is locked".to_string())?
    };
    let mut vault = state.vault.lock().await;
    vault
        .add_key(&passphrase, meta.clone(), &private)
        .map_err(err)?;
    Ok(meta)
}

#[tauri::command]
pub async fn import_key(
    state: State<'_, Arc<AppState>>,
    label: String,
    openssh_private: String,
    key_passphrase: Option<String>,
) -> ApiResult<KeyMeta> {
    let (mut meta, _pair) =
        parse_openssh_private(&openssh_private, key_passphrase.as_deref()).map_err(err)?;
    meta.label = label;
    let passphrase = {
        let pw = state.passphrase.lock().await;
        pw.as_ref()
            .map(|p| p.to_string())
            .ok_or_else(|| "vault is locked".to_string())?
    };
    let mut vault = state.vault.lock().await;
    vault
        .add_key(&passphrase, meta.clone(), &openssh_private)
        .map_err(err)?;
    Ok(meta)
}

#[tauri::command]
pub async fn delete_key(
    state: State<'_, Arc<AppState>>,
    key_id: Uuid,
) -> ApiResult<()> {
    let passphrase = {
        let pw = state.passphrase.lock().await;
        pw.as_ref()
            .map(|p| p.to_string())
            .ok_or_else(|| "vault is locked".to_string())?
    };
    let mut vault = state.vault.lock().await;
    vault
        .remove_key(&passphrase, &key_id.to_string())
        .map_err(err)
}

// ============================================================
// Known Hosts
// ============================================================

#[derive(serde::Serialize)]
pub struct KnownHostEntry {
    pub host: String,
    pub key_type: String,
    pub fingerprint: String,
}

#[tauri::command]
pub async fn list_known_hosts(
    state: State<'_, Arc<AppState>>,
) -> ApiResult<Vec<KnownHostEntry>> {
    let kh = state.known_hosts.lock().await;
    Ok(kh
        .list()
        .into_iter()
        .map(|(host, key_type, fingerprint)| KnownHostEntry {
            host,
            key_type,
            fingerprint,
        })
        .collect())
}

#[tauri::command]
pub async fn remove_known_host(
    state: State<'_, Arc<AppState>>,
    host: String,
    port: u16,
) -> ApiResult<()> {
    let mut kh = state.known_hosts.lock().await;
    kh.remove(&host, port).map_err(err)
}

// ============================================================
// Workspaces
// ============================================================

#[tauri::command]
pub async fn list_workspaces(state: State<'_, Arc<AppState>>) -> ApiResult<Vec<Workspace>> {
    let store = state.store.lock().await;
    Ok(store.workspaces().to_vec())
}

#[tauri::command]
pub async fn create_workspace(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> ApiResult<Workspace> {
    let ws = Workspace::new(name);
    let mut store = state.store.lock().await;
    store.add_workspace(ws.clone()).map_err(err)?;
    Ok(ws)
}

#[tauri::command]
pub async fn save_workspace(
    state: State<'_, Arc<AppState>>,
    workspace: Workspace,
) -> ApiResult<Workspace> {
    let mut store = state.store.lock().await;
    store.update_workspace(workspace.clone()).map_err(err)?;
    Ok(workspace)
}

#[tauri::command]
pub async fn delete_workspace(
    state: State<'_, Arc<AppState>>,
    id: Uuid,
) -> ApiResult<()> {
    let mut store = state.store.lock().await;
    store.remove_workspace(id).map_err(err)
}

#[tauri::command]
pub async fn set_active_workspace(
    state: State<'_, Arc<AppState>>,
    id: Uuid,
) -> ApiResult<()> {
    let mut store = state.store.lock().await;
    store.set_active_workspace(id).map_err(err)
}

// ============================================================
// Sessions (SSH + Local Terminal)
// ============================================================

/// Non-secret metadata from the same immutable identity used to authenticate.
#[derive(Debug, serde::Serialize)]
pub struct SshConnectionInfo {
    pub username: String,
    pub hostname: String,
    pub port: u16,
}

fn ssh_connection_info(
    host: &Host,
    identity: Option<&Identity>,
    expected_username: Option<&str>,
) -> ApiResult<SshConnectionInfo> {
    let username = identity.map(|item| item.username.as_str()).unwrap_or(&host.username);
    // Reject changed accounts before submitting credentials or opening a network connection.
    if expected_username.is_some_and(|expected| expected != username) {
        return Err("SSH identity changed. Reload identities and reconnect to review the account.".into());
    }
    Ok(SshConnectionInfo {
        username: username.to_owned(),
        hostname: host.hostname.clone(),
        port: host.port,
    })
}

#[tauri::command]
pub async fn connect_ssh(
    state: State<'_, Arc<AppState>>,
    _app: AppHandle,
    session_id: String,
    host: Host,
    password: Option<String>,
    cols: Option<u32>,
    rows: Option<u32>,
    expected_username: Option<String>,
) -> ApiResult<SshConnectionInfo> {
    // The owned input is transient and wiped on every exit; never persist or log it.
    let password = password.map(zeroize::Zeroizing::new);
    let passphrase = {
        let pw = state.passphrase.lock().await;
        pw.as_ref().map(|p| p.to_string())
    };

    // Resolve identity if the host references one
    let identity = if let Some(identity_id) = host.identity_id {
        let store = state.store.lock().await;
        store
            .data()
            .identities
            .iter()
            .find(|i| i.id == identity_id)
            .cloned()
    } else {
        None
    };
    let info = ssh_connection_info(&host, identity.as_ref(), expected_username.as_deref())?;

    // Determine if we need the vault (publickey auth from host or identity)
    let needs_vault = match identity.as_ref() {
        Some(id) => matches!(id.auth, AuthMethod::PublicKey),
        None => matches!(host.auth, AuthMethod::PublicKey),
    };
    let vault = state.vault.lock().await;
    let vault_ref = if needs_vault { Some(&*vault) } else { None };
    let known_hosts = state.known_hosts.clone();
    let cols = cols.unwrap_or(80);
    let rows = rows.unwrap_or(24);

    state
        .sessions
        .create_ssh_session(
            session_id,
            &host,
            identity.as_ref(),
            known_hosts,
            vault_ref,
            passphrase.as_deref(),
            password.as_ref().map(|value| value.as_str()),
            cols,
            rows,
        )
        .await
        .map_err(err)?;
    Ok(info)
}

/// Upper bound on local terminals held open at once. Each one owns a PTY, a
/// child shell and a reader thread, so an unbounded map is an unbounded
/// resource claim on the machine.
const MAX_LOCAL_TERMINALS: usize = 16;

/// Decide whether a new local terminal may take `session_id`.
///
/// `session_write`, `session_resize` and the `session-data` event stream all
/// address sessions by id alone, so a second session on a live id silently
/// repoints one of those channels at the other session — for a local id the
/// previous entry is dropped, hanging up its shell, and for an SSH id writes
/// keep going to SSH while the new PTY's output is rendered in the SSH pane.
/// Refuse the collision here instead of resolving it arbitrarily.
///
/// This guards local-terminal creation only. `connect_ssh` performs no such
/// check and `SessionManager::create_ssh_session` inserts unconditionally, so
/// an SSH session can still be opened on an id a local terminal already holds,
/// or on top of another SSH session. Uniqueness is therefore a property of this
/// entry point, not an invariant of session ids in general.
fn admit_local_terminal(
    session_id: &str,
    locals: &LocalTerminals,
    ssh_session_ids: &[String],
) -> ApiResult<()> {
    if locals.contains(session_id) {
        return Err("session id is already in use by a local terminal".into());
    }
    if ssh_session_ids.iter().any(|id| id == session_id) {
        return Err("session id is already in use by an SSH session".into());
    }
    if locals.len() >= MAX_LOCAL_TERMINALS {
        return Err(format!(
            "too many local terminals open (limit {MAX_LOCAL_TERMINALS}); close one first"
        ));
    }
    Ok(())
}

/// Whether a local terminal's shell has finished, given the result of
/// `Child::try_wait`.
///
/// A child whose status cannot be read counts as live. A failed read is not
/// evidence that the shell exited, and treating it as one would discard a
/// terminal the user is still typing into.
fn local_shell_exited(status: std::io::Result<Option<portable_pty::ExitStatus>>) -> bool {
    matches!(status, Ok(Some(_)))
}

#[tauri::command]
pub async fn create_local_terminal(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    session_id: String,
    cols: Option<u32>,
    rows: Option<u32>,
) -> ApiResult<()> {
    use portable_pty::*;

    // `SessionManager::list` takes and releases its own lock and returns a copy,
    // so this snapshot is stale by the time the local map is locked: the guard
    // below covers the local map and the cap, not the SSH half of the check.
    // Taking it first is deliberate — every site that touches both maps drops
    // the `sessions` guard before taking `local_terminals`, and nesting them
    // here would be the only place with the opposite order.
    let ssh_session_ids = state.sessions.list().await;
    let (dead, admission) = {
        let mut locals = state.local_terminals.lock().await;
        // An entry outlives its shell: nothing removes it before `close_session`
        // or the pane unmounting, so panes left sitting disconnected keep their
        // slots. Reaping first makes the cap bound live shells rather than map
        // entries.
        let dead = locals.reap_exited(|term| local_shell_exited(term.child.try_wait()));
        let admission = admit_local_terminal(&session_id, &locals, &ssh_session_ids);
        // Claim the id and the slot before releasing the lock. The PTY and the
        // shell below are slow and blocking, so they cannot be started under it;
        // checking here and inserting afterwards would let every request in a
        // burst pass the check and spawn, with the surplus refused only once the
        // processes already existed.
        if admission.is_ok() {
            locals.reserve(session_id.clone());
        }
        (dead, admission)
    };
    // Tearing down the reaped PTYs can block, so it happens with the lock
    // released rather than in front of every other pane's keystrokes.
    drop(dead);
    admission?;

    // From here every failure has to give the reservation back, or the slot
    // stays claimed for a terminal that will never exist.
    macro_rules! release_on_err {
        ($result:expr, $context:literal) => {
            match $result {
                Ok(value) => value,
                Err(e) => {
                    state.local_terminals.lock().await.release(&session_id);
                    return Err(format!(concat!($context, ": {}"), e));
                }
            }
        };
    }

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: rows.unwrap_or(24) as u16,
        cols: cols.unwrap_or(80) as u16,
        pixel_width: 0,
        pixel_height: 0,
    });
    let pair = release_on_err!(pair, "openpty");

    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "windows") {
            "powershell.exe".to_string()
        } else {
            "/bin/bash".to_string()
        }
    });

    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");

    let child = release_on_err!(pair.slave.spawn_command(cmd), "spawn");

    let mut reader = release_on_err!(pair.master.try_clone_reader(), "clone reader");

    let writer = release_on_err!(pair.master.take_writer(), "take writer");

    let master = pair.master;

    // Drop the slave so the child process owns the only slave fd.
    // On Unix this is the correct pattern — the child has its own copy.
    drop(pair.slave);

    // The lock is taken again to turn the reservation into a live terminal. The
    // id and the slot were claimed before any of the blocking work above, so
    // neither can have been taken in the meantime; only the SSH half of the
    // check can have changed, because `connect_ssh` never consults the local
    // map and so cannot be held off by a reservation.
    let ssh_session_ids = state.sessions.list().await;
    let terminal = LocalTerminal {
        writer,
        master,
        child,
    };
    let refused = {
        let mut locals = state.local_terminals.lock().await;
        if ssh_session_ids.iter().any(|id| id == &session_id) {
            locals.release(&session_id);
            Some((
                "session id is already in use by an SSH session".to_string(),
                terminal,
            ))
        } else {
            // A reservation that is no longer there means the session was closed
            // while its shell was starting, so this terminal is not wanted.
            match locals.fulfil(&session_id, terminal) {
                Ok(()) => None,
                Err(unwanted) => Some((
                    "session was closed while the terminal was starting".to_string(),
                    unwanted,
                )),
            }
        }
    };
    if let Some((error, _refused)) = refused {
        // Torn down with the lock released, for the same reason the reaped
        // entries are.
        return Err(error);
    }

    // Started after the insert: a terminal refused above must not emit
    // `session-closed` for an id that belongs to another session.
    let app_handle = app.clone();
    let sid = session_id;
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = app_handle.emit(
                        "session-data",
                        crate::state::SessionDataEvent {
                            session_id: sid.clone(),
                            data: buf[..n].to_vec(),
                        },
                    );
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(_) => break,
            }
        }
        let _ = app_handle.emit(
            "session-closed",
            crate::state::SessionClosedEvent {
                session_id: sid,
                reason: "local terminal closed".to_string(),
            },
        );
    });

    Ok(())
}

#[tauri::command]
pub async fn session_write(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    data: Vec<u8>,
) -> ApiResult<()> {
    // Try SSH session first
    if state.sessions.list().await.contains(&session_id) {
        return state.sessions.write(&session_id, &data).await.map_err(err);
    }
    // Try local terminal
    let mut locals = state.local_terminals.lock().await;
    if let Some(term) = locals.get_live_mut(&session_id) {
        use std::io::Write;
        term.writer.write_all(&data).map_err(|e| format!("write: {e}"))?;
        term.writer.flush().map_err(|e| format!("flush: {e}"))?;
        return Ok(());
    }
    Err("session not found".into())
}

#[tauri::command]
pub async fn session_resize(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> ApiResult<()> {
    // Try SSH session first
    if state.sessions.list().await.contains(&session_id) {
        return state.sessions.resize(&session_id, cols, rows).await.map_err(err);
    }
    // Try local terminal
    let locals = state.local_terminals.lock().await;
    if let Some(term) = locals.get_live(&session_id) {
        use portable_pty::PtySize;
        term.master
            .resize(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("resize: {e}"))?;
        return Ok(());
    }
    Err("session not found".into())
}

#[tauri::command]
pub async fn close_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> ApiResult<()> {
    // Try SSH session
    if state.sessions.list().await.contains(&session_id) {
        return state.sessions.close(&session_id).await.map_err(err);
    }
    // Try local terminal
    let mut locals = state.local_terminals.lock().await;
    locals.remove(&session_id);
    Ok(())
}

#[tauri::command]
pub async fn list_sessions(state: State<'_, Arc<AppState>>) -> ApiResult<Vec<String>> {
    let mut sessions = state.sessions.list().await;
    let locals = state.local_terminals.lock().await;
    sessions.extend(locals.live_ids().cloned());
    Ok(sessions)
}

// ============================================================
// SFTP (File Browser)
// ============================================================

#[tauri::command]
pub async fn sftp_connect(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    host: Host,
    password: Option<String>,
    expected_username: Option<String>,
) -> ApiResult<()> {
    // Match terminal SSH: do not retain the owned password after this command exits.
    let password = password.map(zeroize::Zeroizing::new);
    let passphrase = {
        let pw = state.passphrase.lock().await;
        pw.as_ref().map(|p| p.to_string())
    };

    let identity = if let Some(identity_id) = host.identity_id {
        let store = state.store.lock().await;
        store
            .data()
            .identities
            .iter()
            .find(|i| i.id == identity_id)
            .cloned()
    } else {
        None
    };

    // Re-resolve the linked identity from the native store and reject an account
    // change before any network/authentication work can consume this credential.
    ssh_connection_info(&host, identity.as_ref(), expected_username.as_deref())?;

    let needs_vault = match identity.as_ref() {
        Some(id) => matches!(id.auth, AuthMethod::PublicKey),
        None => matches!(host.auth, AuthMethod::PublicKey),
    };
    let vault = state.vault.lock().await;
    let vault_ref = if needs_vault { Some(&*vault) } else { None };
    let known_hosts = state.known_hosts.clone();

    state
        .sftp
        .connect(
            session_id,
            &host,
            identity.as_ref(),
            known_hosts,
            vault_ref,
            passphrase.as_deref(),
            password.as_ref().map(|value| value.as_str()),
        )
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn sftp_list_dir(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> ApiResult<Vec<SftpEntry>> {
    state.sftp.list_dir(&session_id, &path).await.map_err(err)
}

#[tauri::command]
pub async fn sftp_canonicalize(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> ApiResult<String> {
    state
        .sftp
        .canonicalize(&session_id, &path)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn sftp_read_file(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> ApiResult<Vec<u8>> {
    state.sftp.read_file(&session_id, &path).await.map_err(err)
}

#[tauri::command]
pub async fn sftp_write_file(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
    data: Vec<u8>,
) -> ApiResult<()> {
    state
        .sftp
        .write_file(&session_id, &path, &data)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn sftp_create_dir(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> ApiResult<()> {
    state
        .sftp
        .create_dir(&session_id, &path)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn sftp_remove_file(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> ApiResult<()> {
    state
        .sftp
        .remove_file(&session_id, &path)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn sftp_remove_dir(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> ApiResult<()> {
    state
        .sftp
        .remove_dir(&session_id, &path)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    old_path: String,
    new_path: String,
) -> ApiResult<()> {
    state
        .sftp
        .rename(&session_id, &old_path, &new_path)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn sftp_close(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> ApiResult<()> {
    state.sftp.close(&session_id).await.map_err(err)
}

// ============================================================
// File I/O
// ============================================================

/// Read a private key file from the filesystem. Used by the key import
/// dialog when the user browses for a file instead of pasting.
#[tauri::command]
pub async fn read_key_file(path: String) -> ApiResult<String> {
    // Validate the path looks like a key file (basic sanity check)
    let path = std::path::Path::new(&path);
    if !path.is_file() {
        return Err(format!("Not a file: {}", path.display()));
    }
    // Limit file size to 256KB to prevent reading huge files
    let metadata = std::fs::metadata(path).map_err(err)?;
    if metadata.len() > 256 * 1024 {
        return Err("File too large (max 256KB)".to_string());
    }
    let content = std::fs::read_to_string(path).map_err(err)?;
    Ok(content)
}

// ============================================================
// Updater
// ============================================================

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub version: String,
    pub current_version: String,
    pub date: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub arch: String,
}

/// GitHub API release representation (subset of fields we need).
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// The GitHub repo to check for releases. Hardcoded for now; could be
/// made configurable later.
const GITHUB_API_RELEASES_URL: &str =
    "https://api.github.com/repos/CodeAny-inc/Clavyn/releases?per_page=30";

/// Find the URL of the `latest.json` asset from the newest release
/// (including prereleases). Returns `None` if no releases have one.
///
/// We query the GitHub API (which includes prereleases, unlike the
/// `releases/latest` redirect) and pick the release with the highest
/// semver version that has a `latest.json` asset.
async fn find_latest_json_url() -> Result<Option<String>, String> {
    let client = reqwest::Client::builder()
        .user_agent("Clavyn-Updater")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .get(GITHUB_API_RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("github api request: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("github api returned {}", resp.status()));
    }

    let releases: Vec<GithubRelease> = resp
        .json()
        .await
        .map_err(|e| format!("parse github response: {e}"))?;

    // Find the release with the highest semver version that has a latest.json
    let mut best: Option<(semver::Version, String)> = None;
    for release in &releases {
        // Strip leading 'v' from tag name
        let tag = release.tag_name.trim_start_matches('v');
        let Ok(version) = semver::Version::parse(tag) else {
            continue;
        };
        // Find the latest.json asset
        let json_url = release
            .assets
            .iter()
            .find(|a| a.name == "latest.json")
            .map(|a| a.browser_download_url.clone());
        let Some(json_url) = json_url else {
            continue;
        };
        match &best {
            Some((best_ver, _)) if &version <= best_ver => {}
            _ => best = Some((version, json_url)),
        }
    }

    Ok(best.map(|(_, url)| url))
}

/// Build an updater with a custom endpoint (the latest prerelease's
/// latest.json URL) and check for updates.
pub async fn check_with_prerelease_endpoint(
    app: &AppHandle,
) -> Result<Option<tauri_plugin_updater::Update>, String> {
    use tauri_plugin_updater::UpdaterExt;

    let json_url = match find_latest_json_url().await? {
        Some(url) => url,
        None => {
            // No releases with latest.json — fall back to the configured endpoint
            let updater = app.updater().map_err(|e| e.to_string())?;
            return updater.check().await.map_err(|e| e.to_string());
        }
    };

    tracing::info!("updater: using latest.json from {json_url}");

    let parsed_url: url::Url = json_url
        .parse()
        .map_err(|e: url::ParseError| e.to_string())?;
    let updater = app
        .updater_builder()
        .endpoints(vec![parsed_url])
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;

    let current_version = app.package_info().version.clone();
    tracing::info!("updater: current version = {current_version}");

    let result = updater.check().await;
    match &result {
        Ok(Some(update)) => {
            tracing::info!("updater: update available v{}", update.version);
        }
        Ok(None) => {
            tracing::info!("updater: no update available (current: {current_version})");
        }
        Err(e) => {
            tracing::warn!("updater: check failed: {e}");
        }
    }
    result.map_err(|e| e.to_string())
}

/// Get app information (name, version, platform).
#[tauri::command]
pub fn get_app_info(app: AppHandle) -> AppInfo {
    let pkg = app.package_info();
    AppInfo {
        name: pkg.name.clone(),
        version: pkg.version.to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

/// Check for updates. Returns update info if an update is available.
///
/// Unlike the default Tauri updater (which uses `releases/latest` and
/// thus skips prereleases), this queries the GitHub API to find the
/// newest release — including prereleases — and uses its `latest.json`.
#[tauri::command]
pub async fn check_for_updates(
    app: AppHandle,
) -> ApiResult<UpdateInfo> {
    tracing::info!("check_for_updates command invoked");
    let current = app.package_info().version.to_string();

    match check_with_prerelease_endpoint(&app).await {
        Ok(Some(update)) => Ok(UpdateInfo {
            available: true,
            version: update.version.clone(),
            current_version: current,
            date: update.date.map(|d| d.to_string()),
            body: update.body.clone(),
        }),
        Ok(None) => Ok(UpdateInfo {
            available: false,
            version: current.clone(),
            current_version: current,
            date: None,
            body: None,
        }),
        Err(e) => {
            tracing::warn!("update check failed: {e}");
            Err(e)
        }
    }
}

/// Download and install the update, then restart the app.
/// Emits "update-progress" events with download progress.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
) -> ApiResult<()> {
    let update = check_with_prerelease_endpoint(&app)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("No update available")?;

    // Download and install
    update
        .download_and_install(
            |chunk_length, content_length| {
                let _ = app.emit(
                    "update-progress",
                    serde_json::json!({
                        "chunk_length": chunk_length,
                        "content_length": content_length,
                    }),
                );
            },
            || {
                let _ = app.emit("update-extracting", serde_json::json!({}));
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    // Restart the app to apply the update
    app.request_restart();

    Ok(())
}

#[cfg(test)]
mod admit_local_terminal_tests {
    use super::*;

    /// `count` terminals mid-open. A reservation is how a caller holds a slot
    /// while its shell starts, and it is the only occupancy a test can build:
    /// a live entry owns a real PTY.
    fn opening(count: usize) -> LocalTerminals {
        let mut locals = LocalTerminals::default();
        for i in 0..count {
            locals.reserve(format!("local-{i}"));
        }
        locals
    }

    #[test]
    fn accepts_a_fresh_id_while_slots_remain() {
        assert!(admit_local_terminal("fresh", &opening(1), &["ssh-a".to_string()]).is_ok());
    }

    #[test]
    fn rejects_an_id_already_held_by_a_local_terminal() {
        let error = admit_local_terminal("local-0", &opening(2), &[]).unwrap_err();
        assert!(error.contains("local terminal"), "{error}");
    }

    #[test]
    fn rejects_an_id_already_held_by_an_ssh_session() {
        let error =
            admit_local_terminal("ssh-a", &LocalTerminals::default(), &["ssh-a".to_string()])
                .unwrap_err();
        assert!(error.contains("SSH session"), "{error}");
    }

    #[test]
    fn rejects_a_fresh_id_once_the_concurrent_limit_is_reached() {
        assert!(admit_local_terminal("fresh", &opening(MAX_LOCAL_TERMINALS - 1), &[]).is_ok());
        let error = admit_local_terminal("fresh", &opening(MAX_LOCAL_TERMINALS), &[]).unwrap_err();
        assert!(error.contains("too many local terminals"), "{error}");
    }

    #[test]
    fn reports_the_collision_rather_than_the_limit_when_both_apply() {
        let error =
            admit_local_terminal("local-0", &opening(MAX_LOCAL_TERMINALS), &[]).unwrap_err();
        assert!(error.contains("local terminal"), "{error}");
    }

    /// The window this closes: the shell is spawned with the lock released, so
    /// a burst that only checked the cap would have every request pass before
    /// any of them inserted, and the surplus would be refused with the
    /// processes already running. Occupancy has to be claimed by the check
    /// itself.
    #[test]
    fn a_burst_of_admissions_claims_slots_as_it_goes_rather_than_all_passing() {
        let mut locals = LocalTerminals::default();
        let mut admitted = 0;
        for i in 0..MAX_LOCAL_TERMINALS * 2 {
            let id = format!("burst-{i}");
            if admit_local_terminal(&id, &locals, &[]).is_ok() {
                locals.reserve(id);
                admitted += 1;
            }
        }
        assert_eq!(admitted, MAX_LOCAL_TERMINALS);
        assert_eq!(locals.len(), MAX_LOCAL_TERMINALS);
    }

    /// A reservation that is never fulfilled has to free its slot, or a failed
    /// spawn would shrink the cap for the rest of the session.
    #[test]
    fn releasing_a_reservation_returns_its_slot() {
        let mut locals = opening(MAX_LOCAL_TERMINALS);
        assert!(admit_local_terminal("fresh", &locals, &[]).is_err());
        locals.release("local-0");
        assert!(admit_local_terminal("fresh", &locals, &[]).is_ok());
        assert!(admit_local_terminal("local-0", &locals, &[]).is_ok());
    }

    /// A reserved id is claimed but not usable yet: `session_write` and
    /// `list_sessions` must not see it, or the UI would address a terminal
    /// whose shell does not exist.
    #[test]
    fn a_reserved_id_is_claimed_but_not_yet_addressable() {
        let locals = opening(2);
        assert!(locals.contains("local-0"));
        assert!(locals.get_live("local-0").is_none());
        assert_eq!(locals.live_ids().count(), 0);
    }
}

#[cfg(test)]
mod local_terminal_reaping_tests {
    use super::*;
    use portable_pty::ExitStatus;

    #[test]
    fn a_child_that_reported_an_exit_is_finished() {
        assert!(local_shell_exited(Ok(Some(ExitStatus::with_exit_code(0)))));
        assert!(local_shell_exited(Ok(Some(ExitStatus::with_exit_code(1)))));
    }

    #[test]
    fn a_child_that_has_not_exited_is_live() {
        assert!(!local_shell_exited(Ok(None)));
    }

    #[test]
    fn a_child_whose_status_cannot_be_read_is_live() {
        let unreadable = std::io::Error::new(std::io::ErrorKind::Other, "status unavailable");
        assert!(!local_shell_exited(Err(unreadable)));
    }
}

#[cfg(test)]
mod ssh_connection_info_tests {
    use super::*;

    fn host() -> Host {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::nil(), "label": "Fixture", "hostname": "server.example.test",
            "port": 22, "username": "deploy", "auth": "agent", "tags": []
        })).unwrap()
    }
    fn identity() -> Identity {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::nil(), "label": "Fixture identity", "username": "root",
            "auth": "agent", "tags": []
        })).unwrap()
    }

    #[test]
    fn snapshots_the_resolved_identity_not_the_host_fallback() {
        let host = host();
        let mut identity = identity();
        let info = ssh_connection_info(&host, Some(&identity), Some("root")).unwrap();
        identity.username = "ops".into();
        assert_eq!(info.username, "root");
        assert_eq!(info.hostname, "server.example.test");
        assert_eq!(info.port, 22);
        assert_eq!(host.username, "deploy");
    }

    #[test]
    fn supports_missing_identity_fallback_and_legacy_callers() {
        assert_eq!(ssh_connection_info(&host(), None, Some("deploy")).unwrap().username, "deploy");
        assert_eq!(ssh_connection_info(&host(), Some(&identity()), None).unwrap().username, "root");
    }

    #[test]
    fn rejects_changed_identity_before_a_network_connection_can_start() {
        let result = ssh_connection_info(&host(), Some(&identity()), Some("deploy"));
        assert!(result.unwrap_err().contains("SSH identity changed"));
    }
}
