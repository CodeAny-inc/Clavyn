use crate::host_key_prompt;
use crate::state::{AppState, LocalTerminal};
use clavyn_core::host::{AuthMethod, Host, HostGroup};
use clavyn_core::identity::Identity;
use clavyn_core::keys::{generate_ed25519, parse_openssh_private, KeyMeta};
use clavyn_core::known_hosts::{HostKeyChange, KnownHosts};
use clavyn_core::sftp::SftpEntry;
use clavyn_core::workspace::Workspace;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
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

/// Stop trusting a host. No native confirmation: `remove` tombstones the entry
/// and keeps its key, so the host stays protected by the mismatch check and
/// this call on its own cannot downgrade anything. Erasing the retained key is
/// `forget_known_host`, and that one does ask.
#[tauri::command]
pub async fn remove_known_host(
    state: State<'_, Arc<AppState>>,
    host: String,
    port: u16,
) -> ApiResult<()> {
    let mut kh = state.known_hosts.lock().await;
    kh.remove(&host, port).map_err(err)
}

/// Hosts with no live pin whose last key is still retained, so a different key
/// is reported as a change. Listed separately from the trusted hosts, because
/// what the app retains has to be visible to be erasable.
#[tauri::command]
pub async fn list_removed_known_hosts(
    state: State<'_, Arc<AppState>>,
) -> ApiResult<Vec<KnownHostEntry>> {
    let kh = state.known_hosts.lock().await;
    Ok(kh
        .removed()
        .into_iter()
        .map(|(host, key_type, fingerprint)| KnownHostEntry {
            host,
            key_type,
            fingerprint,
        })
        .collect())
}

/// Erase the key retained for a tombstoned host. That puts the host back on
/// trust-on-first-use, so it is confirmed in a native dialog that prints the
/// fingerprint about to be erased: the webview can reach this command, but it
/// cannot answer for the user.
#[tauri::command]
pub async fn forget_known_host(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    host: String,
    port: u16,
) -> ApiResult<()> {
    let state = state.inner().clone();
    let gate = state.clone();
    let label = format!("{host}:{port}");
    forget_confirmed(&state.known_hosts, &host, port, move |fingerprint| async move {
        host_key_prompt::confirm(
            &app,
            &gate.host_key_prompt,
            "Forget host key",
            &host_key_prompt::forget_message(&label, &fingerprint),
            "Forget the key",
        )
        .await
    })
    .await
}

/// `forget_known_host` without the dialog, so the ordering the command relies on
/// can be tested: read the retained fingerprint, confirm it, then check that it
/// is still the one on record before erasing anything.
///
/// The lock is taken twice and never held across the question — a dialog can
/// stay open for minutes, and the store has to keep serving connections — so
/// each half has to stand on its own. `forgettable_fingerprint` decides and
/// refuses without touching anything, which keeps the branch that asks nothing
/// from being a branch that erases something; the recheck after the dialog is
/// what makes the second acquisition safe.
async fn forget_confirmed<F, Fut>(
    known_hosts: &Mutex<KnownHosts>,
    host: &str,
    port: u16,
    confirm: F,
) -> ApiResult<()>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = ApiResult<()>>,
{
    // Nothing retained means nothing to confirm, and the refusal says why: a
    // live pin has to be removed first, and an unknown host was never here.
    let fingerprint = known_hosts
        .lock()
        .await
        .forgettable_fingerprint(host, port)
        .map_err(err)?;
    confirm(fingerprint.clone()).await?;
    let mut kh = known_hosts.lock().await;
    if kh.retained_fingerprint(host, port).as_deref() != Some(fingerprint.as_str()) {
        return Err(format!(
            "the key retained for {host}:{port} changed while the confirmation was open; review it again"
        ));
    }
    kh.forget(host, port).map_err(err)
}

#[derive(serde::Serialize)]
pub struct PendingHostKeyChange {
    pub host: String,
    pub key_type: String,
    pub pinned_fingerprint: String,
    pub presented_fingerprint: String,
}

#[tauri::command]
pub async fn list_host_key_changes(
    state: State<'_, Arc<AppState>>,
) -> ApiResult<Vec<PendingHostKeyChange>> {
    let kh = state.known_hosts.lock().await;
    Ok(kh
        .pending_changes()
        .into_iter()
        .map(|change| PendingHostKeyChange {
            host: change.host,
            key_type: change.key_type,
            pinned_fingerprint: change.pinned_fingerprint,
            presented_fingerprint: change.presented_fingerprint,
        })
        .collect())
}

/// Pin the key a server presented in place of the recorded one. `fingerprint`
/// is the value the caller displayed: it is checked against the held key so a
/// view rendered before another key arrived cannot trust an unreviewed one.
///
/// That check is a guard against a stale view, not against a script, so the pin
/// itself is confirmed in a native dialog that prints both fingerprints as the
/// store holds them.
#[tauri::command]
pub async fn replace_known_host(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    host: String,
    port: u16,
    fingerprint: String,
) -> ApiResult<()> {
    let state = state.inner().clone();
    let gate = state.clone();
    replace_confirmed(&state.known_hosts, &host, port, &fingerprint, move |change| async move {
        host_key_prompt::confirm(
            &app,
            &gate.host_key_prompt,
            "Host key changed",
            &host_key_prompt::trust_message(
                &change.host,
                &change.pinned_fingerprint,
                &change.presented_fingerprint,
            ),
            "Trust the new key",
        )
        .await
    })
    .await
}

/// `replace_known_host` without the dialog. The change handed to `confirm` comes
/// from the held key, so what is shown is what the server presented rather than
/// what the caller passed.
///
/// Same two halves as `forget_confirmed`, and the same reason for them: the
/// question is decided without the power to answer it, and what the dialog
/// showed is compared with what the store holds before anything is pinned. Both
/// fingerprints are checked, not just the presented one — a pin that moved while
/// the dialog was open means the user was comparing against a record that is no
/// longer there.
async fn replace_confirmed<F, Fut>(
    known_hosts: &Mutex<KnownHosts>,
    host: &str,
    port: u16,
    fingerprint: &str,
    confirm: F,
) -> ApiResult<()>
where
    F: FnOnce(HostKeyChange) -> Fut,
    Fut: std::future::Future<Output = ApiResult<()>>,
{
    // Either no key is being held or the caller named a different one: both are
    // refused here, because asking about a key that is not going to be pinned
    // would only train the user to dismiss the dialog.
    let change = known_hosts
        .lock()
        .await
        .confirmable_change(host, port, fingerprint)
        .map_err(err)?;
    let shown = change.clone();
    confirm(change).await?;
    let mut kh = known_hosts.lock().await;
    let current = kh.confirmable_change(host, port, fingerprint).map_err(err)?;
    if current.pinned_fingerprint != shown.pinned_fingerprint
        || current.presented_fingerprint != shown.presented_fingerprint
    {
        return Err(format!(
            "the host keys for {host}:{port} changed while the confirmation was open; review them again"
        ));
    }
    kh.trust_presented_key(host, port, fingerprint).map_err(err)
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
    mut workspace: Workspace,
) -> ApiResult<Workspace> {
    // Sanitize before the store does, so the caller gets back the layout that
    // was actually persisted rather than the one it sent.
    workspace.sanitize().map_err(err)?;
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

#[tauri::command]
pub async fn create_local_terminal(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    session_id: String,
    cols: Option<u32>,
    rows: Option<u32>,
) -> ApiResult<()> {
    use portable_pty::*;

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.unwrap_or(24) as u16,
            cols: cols.unwrap_or(80) as u16,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty: {e}"))?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "windows") {
            "powershell.exe".to_string()
        } else {
            "/bin/bash".to_string()
        }
    });

    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn: {e}"))?;

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone reader: {e}"))?;

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take writer: {e}"))?;

    let master = pair.master;

    // Drop the slave so the child process owns the only slave fd.
    // On Unix this is the correct pattern — the child has its own copy.
    drop(pair.slave);

    // Spawn a reading thread that emits data events
    let app_handle = app.clone();
    let sid = session_id.clone();
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

    // Store the master and child for writing, resizing, and keeping
    // the child process alive.
    let mut locals = state.local_terminals.lock().await;
    locals.insert(
        session_id,
        LocalTerminal {
            writer,
            master,
            _child: child,
        },
    );

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
    if let Some(term) = locals.get_mut(&session_id) {
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
    if let Some(term) = locals.get(&session_id) {
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
    sessions.extend(locals.keys().cloned());
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

    // Release the single-instance guard for the one restart path that never
    // reaches the plugin's own release. `request_restart` normally asks the
    // runtime to exit, which emits `RunEvent::Exit`; the plugin releases the
    // guard from that event, before the replacement process is spawned. When
    // the runtime refuses the exit request, `request_restart` spawns the
    // replacement straight from the caller's thread and no exit event is ever
    // emitted — without this call the successor would find the guard held and
    // exit on startup, leaving no Clavyn running after an update. Releasing an
    // already-released guard is a no-op, so the ordinary path pays nothing.
    // Guarded by the same condition as registration: a build that never
    // claimed the guard must not release one an installed build is holding.
    if crate::single_instance_guard_applies() {
        tauri_plugin_single_instance::destroy(&app);
    }

    // Restart the app to apply the update
    app.request_restart();

    Ok(())
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

/// The command bodies are exercised here with the confirmation stubbed out. The
/// `#[tauri::command]` wrappers add nothing but the native dialog, so what these
/// cover is the ordering the dialog depends on: the fingerprint is read before
/// the question, nothing is written when the answer is no, and a key that moved
/// while the dialog was open is not the one that gets used.
#[cfg(test)]
mod known_host_confirmation_tests {
    use super::{forget_confirmed, replace_confirmed};
    use clavyn_core::known_hosts::KnownHosts;
    use russh_keys::key::{KeyPair, PublicKey};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio::task::JoinSet;

    const HOST: &str = "prod.example.com";
    const PORT: u16 = 22;
    const DECLINED: &str = "Forget host key: cancelled";

    fn server_key() -> PublicKey {
        KeyPair::generate_ed25519()
            .clone_public_key()
            .expect("public key")
    }

    fn store(dir: &std::path::Path) -> Mutex<KnownHosts> {
        Mutex::new(KnownHosts::load(dir.join("known_hosts.json")).expect("load"))
    }

    async fn removed_host(store: &Mutex<KnownHosts>, key: &PublicKey) {
        let mut kh = store.lock().await;
        kh.verify(HOST, PORT, key).expect("first use");
        kh.remove(HOST, PORT).expect("remove");
    }

    #[tokio::test]
    async fn a_declined_confirmation_leaves_the_retained_key_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        let pinned = server_key();
        removed_host(&store, &pinned).await;

        let error = forget_confirmed(&store, HOST, PORT, |_| async { Err(DECLINED.to_string()) })
            .await
            .expect_err("a declined confirmation erased the key anyway");

        assert_eq!(error, DECLINED);
        let kh = store.lock().await;
        assert_eq!(
            kh.retained_fingerprint(HOST, PORT),
            Some(pinned.fingerprint()),
            "the retained key did not survive the refusal"
        );
        // Still remembered, so a different key is reported as a change rather
        // than trusted as a first contact.
        assert!(kh.check_mismatch(HOST, PORT, &server_key()).is_err());
    }

    #[tokio::test]
    async fn forgetting_confirms_the_fingerprint_it_is_about_to_erase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        let pinned = server_key();
        removed_host(&store, &pinned).await;

        let shown = std::cell::RefCell::new(String::new());
        forget_confirmed(&store, HOST, PORT, |fingerprint| {
            *shown.borrow_mut() = fingerprint;
            async { Ok(()) }
        })
        .await
        .expect("forget");

        assert_eq!(shown.into_inner(), pinned.fingerprint());
        assert_eq!(store.lock().await.retained_fingerprint(HOST, PORT), None);
    }

    #[tokio::test]
    async fn a_key_that_changes_while_the_dialog_is_open_is_not_erased() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        removed_host(&store, &server_key()).await;
        let replacement = server_key();

        let error = forget_confirmed(&store, HOST, PORT, |_| async {
            // The host is re-pinned to another key and removed again while the
            // user is still reading the first fingerprint.
            let mut kh = store.lock().await;
            kh.replace(HOST, PORT, &replacement).expect("replace");
            kh.remove(HOST, PORT).expect("remove");
            Ok(())
        })
        .await
        .expect_err("a key nobody was shown was erased");

        assert!(error.contains("changed while the confirmation was open"));
        assert_eq!(
            store.lock().await.retained_fingerprint(HOST, PORT),
            Some(replacement.fingerprint())
        );
    }

    #[tokio::test]
    async fn a_live_pin_is_refused_without_asking_anything() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        store
            .lock()
            .await
            .verify(HOST, PORT, &server_key())
            .expect("first use");

        let error = forget_confirmed(&store, HOST, PORT, |_| async {
            panic!("a live pin should never reach a confirmation");
        })
        .await
        .expect_err("a live pin was forgotten");

        assert!(error.contains("still trusted"));
    }

    #[tokio::test]
    async fn a_declined_confirmation_leaves_the_pin_intact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        let pinned = server_key();
        let presented = server_key();
        {
            let mut kh = store.lock().await;
            kh.verify(HOST, PORT, &pinned).expect("first use");
            kh.hold_presented_key(HOST, PORT, &presented);
        }

        let error = replace_confirmed(&store, HOST, PORT, &presented.fingerprint(), |_| async {
            Err("Host key changed: cancelled".to_string())
        })
        .await
        .expect_err("a declined confirmation pinned the presented key anyway");

        assert_eq!(error, "Host key changed: cancelled");
        let kh = store.lock().await;
        assert!(
            kh.check_mismatch(HOST, PORT, &pinned).is_ok(),
            "the original pin was replaced"
        );
        assert!(kh.check_mismatch(HOST, PORT, &presented).is_err());
    }

    #[tokio::test]
    async fn trusting_confirms_the_fingerprints_the_store_holds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        let pinned = server_key();
        let presented = server_key();
        {
            let mut kh = store.lock().await;
            kh.verify(HOST, PORT, &pinned).expect("first use");
            kh.hold_presented_key(HOST, PORT, &presented);
        }

        let shown = std::cell::RefCell::new(None);
        replace_confirmed(&store, HOST, PORT, &presented.fingerprint(), |change| {
            *shown.borrow_mut() = Some(change);
            async { Ok(()) }
        })
        .await
        .expect("trust");

        let change = shown.into_inner().expect("nothing was confirmed");
        assert_eq!(change.host, "prod.example.com:22");
        assert_eq!(change.pinned_fingerprint, pinned.fingerprint());
        assert_eq!(change.presented_fingerprint, presented.fingerprint());
        assert!(store
            .lock()
            .await
            .check_mismatch(HOST, PORT, &presented)
            .is_ok());
    }

    #[tokio::test]
    async fn a_fingerprint_that_is_not_the_held_one_never_reaches_a_dialog() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        let pinned = server_key();
        {
            let mut kh = store.lock().await;
            kh.verify(HOST, PORT, &pinned).expect("first use");
            kh.hold_presented_key(HOST, PORT, &server_key());
        }

        let error = replace_confirmed(&store, HOST, PORT, &server_key().fingerprint(), |_| async {
            panic!("a key that is not going to be pinned should never be asked about");
        })
        .await
        .expect_err("an unreviewed key was pinned");

        assert!(error.contains("not the one that was reviewed"));
    }

    #[tokio::test]
    async fn a_pin_that_changes_while_the_dialog_is_open_is_not_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        let presented = server_key();
        {
            let mut kh = store.lock().await;
            kh.verify(HOST, PORT, &server_key()).expect("first use");
            kh.hold_presented_key(HOST, PORT, &presented);
        }
        let repinned = server_key();

        let error = replace_confirmed(&store, HOST, PORT, &presented.fingerprint(), |_| async {
            // The host is pinned to a third key while the user is still
            // comparing the two the dialog printed.
            store
                .lock()
                .await
                .replace(HOST, PORT, &repinned)
                .expect("replace");
            Ok(())
        })
        .await
        .expect_err("a key was pinned against a comparison that no longer held");

        assert!(error.contains("changed while the confirmation was open"));
        assert!(
            store
                .lock()
                .await
                .check_mismatch(HOST, PORT, &repinned)
                .is_ok(),
            "the pin made while the dialog was open was overwritten"
        );
    }

    #[tokio::test]
    async fn a_host_removed_while_the_dialog_is_open_is_not_re_pinned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(dir.path());
        let presented = server_key();
        {
            let mut kh = store.lock().await;
            kh.verify(HOST, PORT, &server_key()).expect("first use");
            kh.hold_presented_key(HOST, PORT, &presented);
        }

        let error = replace_confirmed(&store, HOST, PORT, &presented.fingerprint(), |_| async {
            store.lock().await.remove(HOST, PORT).expect("remove");
            Ok(())
        })
        .await
        .expect_err("a tombstoned host was pinned back to a key the dialog never showed");

        // `remove` drops the held key, so there is nothing left to trust: the
        // dialog compared against a pin that is no longer live.
        assert!(error.contains("no unreviewed host key"), "unexpected: {error}");
        assert!(
            store.lock().await.list().is_empty(),
            "a removed host was restored to the trusted list"
        );
    }

    /// The dialog is the only thing between a scripted `invoke` and a host going
    /// back to trust-on-first-use, so "is there anything to confirm?" and the
    /// erase have to be one decision. Read under one lock and acted on under
    /// another they are two: a burst that answers "nothing is retained" against
    /// a live pin, plus a single `remove` landing in the middle of it, leaves
    /// calls holding a stale answer that erase a key nobody was ever shown.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_around_a_removal_cannot_forget_a_key_without_confirming_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(store(dir.path()));
        store
            .lock()
            .await
            .verify(HOST, PORT, &server_key())
            .expect("first use");

        let asked = Arc::new(AtomicUsize::new(0));
        let mut burst = JoinSet::new();
        for i in 0..400 {
            let store = Arc::clone(&store);
            let asked = Arc::clone(&asked);
            burst.spawn(async move {
                // The removal lands mid-burst, so calls that started on either
                // side of it are in flight at the same time.
                if i == 200 {
                    store.lock().await.remove(HOST, PORT).expect("remove");
                    return;
                }
                let _ = forget_confirmed(&store, HOST, PORT, |_| async move {
                    asked.fetch_add(1, Ordering::SeqCst);
                    // Stands in for a user who says no and for a dialog that
                    // never appeared alike: neither may erase anything.
                    Err(DECLINED.to_string())
                })
                .await;
            });
        }
        while burst.join_next().await.is_some() {}

        // Whatever order the calls landed in, the key is still on record, so a
        // brand-new key is a change to review rather than a first contact.
        assert!(
            store
                .lock()
                .await
                .check_mismatch(HOST, PORT, &server_key())
                .is_err(),
            "{HOST} was put back on trust-on-first-use by a burst that confirmed nothing \
             ({} confirmations shown)",
            asked.load(Ordering::SeqCst)
        );
    }

    /// The same split in the pin path, and a worse outcome: the key that wins is
    /// the one the server presented. A man in the middle knows the fingerprint
    /// it is about to offer, so the burst can be armed with it before the key is
    /// ever held.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_around_a_presented_key_cannot_pin_it_without_confirming_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(store(dir.path()));
        let pinned = server_key();
        let presented = server_key();
        store
            .lock()
            .await
            .verify(HOST, PORT, &pinned)
            .expect("first use");

        let fingerprint = presented.fingerprint();
        let asked = Arc::new(AtomicUsize::new(0));
        let mut burst = JoinSet::new();
        for i in 0..400 {
            let store = Arc::clone(&store);
            let asked = Arc::clone(&asked);
            let fingerprint = fingerprint.clone();
            let presented = presented.clone();
            burst.spawn(async move {
                // The connection path holds the changed key mid-burst.
                if i == 200 {
                    store.lock().await.hold_presented_key(HOST, PORT, &presented);
                    return;
                }
                let _ = replace_confirmed(&store, HOST, PORT, &fingerprint, |_| async move {
                    asked.fetch_add(1, Ordering::SeqCst);
                    Err("Host key changed: cancelled".to_string())
                })
                .await;
            });
        }
        while burst.join_next().await.is_some() {}

        let kh = store.lock().await;
        assert!(
            kh.check_mismatch(HOST, PORT, &presented).is_err(),
            "the presented key was pinned by a burst that confirmed nothing \
             ({} confirmations shown)",
            asked.load(Ordering::SeqCst)
        );
        assert!(
            kh.check_mismatch(HOST, PORT, &pinned).is_ok(),
            "the original pin was replaced"
        );
    }
}
