// Tauri command wrappers — thin typed layer over `invoke`.
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Host,
  HostGroup,
  Identity,
  KeyMeta,
  KnownHostEntry,
  PendingHostKeyChange,
  Workspace,
  SessionClosedEvent,
} from "./types";

export const listHosts = () => invoke<Host[]>("list_hosts");
export const addHost = (host: Host) => invoke<Host>("add_host", { host });
export const updateHost = (host: Host) => invoke<Host>("update_host", { host });
export const deleteHost = (id: string) => invoke<void>("delete_host", { id });
export const listGroups = () => invoke<HostGroup[]>("list_groups");
export const addGroup = (name: string) => invoke<HostGroup>("add_group", { name });
export const deleteGroup = (id: string) => invoke<void>("delete_group", { id });
export const listIdentities = () => invoke<Identity[]>("list_identities");
export const addIdentity = (identity: Identity) => invoke<Identity>("add_identity", { identity });
export const updateIdentity = (identity: Identity) => invoke<Identity>("update_identity", { identity });
export const deleteIdentity = (id: string) => invoke<void>("delete_identity", { id });

export const vaultIsInitialized = () => invoke<boolean>("vault_is_initialized");
export const initializeVault = (passphrase: string) => invoke<boolean>("secure_initialize_vault", { passphrase });
/** `acceptOlder` opens a vault copy older than one this device already opened. */
export const unlockVault = (passphrase: string, acceptOlder = false) =>
  invoke<void>("secure_unlock_vault", { passphrase, acceptOlder });
export const lockVault = () => invoke<void>("secure_lock_vault");
export const resetVault = (passphrase: string) => invoke<void>("secure_reset_vault", { passphrase });
export const isVaultUnlocked = () => invoke<boolean>("is_vault_unlocked");

export const biometricAvailable = () => invoke<boolean>("biometric_available");
export const biometricPassphraseStored = () => invoke<boolean>("biometric_passphrase_stored");
export const storeBiometricPassphrase = (passphrase: string) => invoke<void>("store_biometric_passphrase", { passphrase });
export const unlockWithBiometric = () => invoke<boolean>("unlock_with_biometric");
export const clearBiometricPassphrase = () => invoke<void>("clear_biometric_passphrase");

export const listKeys = () => invoke<KeyMeta[]>("list_keys");
export const generateKey = (label: string) => invoke<KeyMeta>("generate_key", { label });
export const importKey = (label: string, opensshPrivate: string, keyPassphrase: string | null) =>
  invoke<KeyMeta>("import_key", { label, opensshPrivate, keyPassphrase });
export const deleteKey = (keyId: string) => invoke<void>("delete_key", { keyId });
export const listKnownHosts = () => invoke<KnownHostEntry[]>("list_known_hosts");
export const removeKnownHost = (host: string, port: number) => invoke<void>("remove_known_host", { host, port });
export const listRemovedKnownHosts = () => invoke<KnownHostEntry[]>("list_removed_known_hosts");
export const forgetKnownHost = (host: string, port: number) => invoke<void>("forget_known_host", { host, port });
export const listHostKeyChanges = () => invoke<PendingHostKeyChange[]>("list_host_key_changes");
export const replaceKnownHost = (host: string, port: number, fingerprint: string) =>
  invoke<void>("replace_known_host", { host, port, fingerprint });
export const listWorkspaces = () => invoke<Workspace[]>("list_workspaces");
export const createWorkspace = (name: string) => invoke<Workspace>("create_workspace", { name });
export const saveWorkspace = (workspace: Workspace) => invoke<Workspace>("save_workspace", { workspace });
export const deleteWorkspace = (id: string) => invoke<void>("delete_workspace", { id });
export const setActiveWorkspace = (id: string) => invoke<void>("set_active_workspace", { id });
/** Opens the native file dialog and returns the picked key file's text, or null when cancelled. */
export const pickKeyFile = () => invoke<string | null>("pick_key_file");

/**
 * Sink a session's terminal output arrives on. Bytes travel as a binary IPC
 * payload, so they never pass through JSON, and each session has its own sink
 * instead of every pane receiving every session's output.
 */
export type SessionOutput = Channel<ArrayBuffer>;

/** Received output that is acknowledged at once rather than on the timer. */
export const OUTPUT_ACK_THRESHOLD = 256 * 1024;
/** Longest received output waits to be acknowledged below the threshold. */
export const OUTPUT_ACK_DELAY_MS = 30;
/** Longest wait between retries of an acknowledgement that failed. */
export const OUTPUT_ACK_MAX_RETRY_MS = 2000;

/**
 * Reports output bytes as received. An SSH session stops reading from its
 * connection while too much of its output is unacknowledged, so a webview that
 * falls behind slows the remote end down rather than letting undelivered
 * output pile up in the IPC layer. Local terminals ignore it.
 */
export const sessionOutputAck = (sessionId: string, bytes: number) =>
  invoke<void>("session_output_ack", { sessionId, bytes });

/**
 * Opens a sink for one session.
 *
 * A batch is never empty, so the backend marks the end of the stream with a
 * zero-length frame. It rides the channel's own ordering index, which means it
 * is delivered behind every batch already sent — including a batch large enough
 * to take the asynchronous transport route. `onEnd` is therefore the only
 * signal that says no more output is coming; the `session-closed` event carries
 * no index and can overtake output that is still in flight.
 *
 * Every batch is acknowledged on receipt, whether or not the pane still wants
 * it: the backend meters what the IPC layer holds, not what the emulator
 * renders. Acknowledgements are coalesced, going out once
 * `OUTPUT_ACK_THRESHOLD` bytes are pending or `OUTPUT_ACK_DELAY_MS` after the
 * first unacknowledged batch, so a trickle of output never leaves credit
 * stranded. Nothing is acknowledged once the stream has ended; the session is
 * gone by then.
 */
export function sessionOutput(
  sessionId: string,
  onData: (bytes: Uint8Array) => void,
  onEnd: () => void,
): SessionOutput {
  const channel: SessionOutput = new Channel();
  let pending = 0;
  let failures = 0;
  let ended = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const stopTimer = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };
  const acknowledge = () => {
    stopTimer();
    const bytes = pending;
    pending = 0;
    if (bytes === 0) return;
    sessionOutputAck(sessionId, bytes).then(
      () => {
        failures = 0;
      },
      () => {
        // Credit that never reaches the backend is never handed back, and a
        // reader that has run out waits for an acknowledgement no further
        // output will trigger. The bytes go back into the count and are
        // retried, backing off to OUTPUT_ACK_MAX_RETRY_MS, for as long as the
        // stream is open: giving up at any point would leave the session
        // paused for good.
        if (ended) return;
        pending += bytes;
        failures += 1;
        stopTimer();
        timer = setTimeout(acknowledge, Math.min(OUTPUT_ACK_DELAY_MS * 2 ** failures, OUTPUT_ACK_MAX_RETRY_MS));
      },
    );
  };
  channel.onmessage = (message) => {
    const bytes = new Uint8Array(message);
    if (bytes.length === 0) {
      ended = true;
      stopTimer();
      pending = 0;
      onEnd();
      return;
    }
    pending += bytes.length;
    if (pending >= OUTPUT_ACK_THRESHOLD) acknowledge();
    else if (timer === null) timer = setTimeout(acknowledge, OUTPUT_ACK_DELAY_MS);
    onData(bytes);
  };
  return channel;
}

export interface SshConnectionInfo { username: string; hostname: string; port: number }
export const connectSsh = (sessionId: string, hostId: string, password: string | null, cols: number, rows: number, onOutput: SessionOutput, expectedUsername?: string) =>
  invoke<SshConnectionInfo>("connect_ssh", { sessionId, hostId, password, cols, rows, expectedUsername, onOutput });
export const createLocalTerminal = (sessionId: string, cols: number, rows: number, onOutput: SessionOutput) =>
  invoke<void>("create_local_terminal", { sessionId, cols, rows, onOutput });
export const sessionWrite = (sessionId: string, data: number[]) => invoke<void>("session_write", { sessionId, data });
export const sessionResize = (sessionId: string, cols: number, rows: number) => invoke<void>("session_resize", { sessionId, cols, rows });
export const closeSession = (sessionId: string) => invoke<void>("close_session", { sessionId });
export function onSessionClosed(cb: (e: SessionClosedEvent) => void): Promise<UnlistenFn> {
  return listen<SessionClosedEvent>("session-closed", (event) => cb(event.payload));
}

export interface SftpEntry {
  name: string;
  long_name: string;
  is_dir: boolean;
  is_file: boolean;
  is_symlink: boolean;
  size: number;
  modified: number | null;
  permissions: number | null;
}

export const sftpConnect = (
  sessionId: string,
  hostId: string,
  password: string | null,
  expectedUsername?: string,
) => invoke<void>("sftp_connect", { sessionId, hostId, password, expectedUsername });
export const sftpListDir = (sessionId: string, path: string) =>
  invoke<SftpEntry[]>("sftp_list_dir", { sessionId, path });
export const sftpCanonicalize = (sessionId: string, path: string) =>
  invoke<string>("sftp_canonicalize", { sessionId, path });
export const sftpCreateDir = (sessionId: string, path: string) =>
  invoke<void>("sftp_create_dir", { sessionId, path });
export const sftpRemoveFile = (sessionId: string, path: string) =>
  invoke<void>("sftp_remove_file", { sessionId, path });
export const sftpRemoveDir = (sessionId: string, path: string) =>
  invoke<void>("sftp_remove_dir", { sessionId, path });
export const sftpRename = (sessionId: string, oldPath: string, newPath: string) =>
  invoke<void>("sftp_rename", { sessionId, oldPath, newPath });
export const sftpClose = (sessionId: string) => invoke<void>("sftp_close", { sessionId });
/** Asks where to save in the native dialog, then downloads. False when cancelled. */
export const sftpDownloadToLocal = (sessionId: string, remotePath: string) =>
  invoke<boolean>("sftp_download_to_local", { sessionId, remotePath });
/** A local file picked for upload: an opaque single-use token and its file name. */
export interface PickedUpload { token: string; name: string }
export const sftpPickUploadFile = () => invoke<PickedUpload | null>("sftp_pick_upload_file");
export const sftpUploadFromLocal = (
  sessionId: string,
  uploadToken: string,
  remotePath: string,
  overwrite: boolean,
) => invoke<void>("sftp_upload_from_local", { sessionId, uploadToken, remotePath, overwrite });

export interface UpdateInfo {
  available: boolean;
  version: string;
  current_version: string;
  date: string | null;
  body: string | null;
}
export interface AppInfo {
  name: string;
  version: string;
  platform: string;
  arch: string;
}
export const getAppInfo = () => invoke<AppInfo>("get_app_info");
export interface UpdateProgress {
  chunk_length: number;
  content_length: number | null;
}
export const checkForUpdates = () => invoke<UpdateInfo>("check_for_updates");
export const installUpdate = () => invoke<void>("install_update");
export function onUpdateProgress(cb: (e: UpdateProgress) => void): Promise<UnlistenFn> {
  return listen<UpdateProgress>("update-progress", (event) => cb(event.payload));
}
export function onUpdateExtracting(cb: () => void): Promise<UnlistenFn> {
  return listen("update-extracting", () => cb());
}

/** What stopped the app state from loading at startup, if anything did. */
export interface StartupFailure {
  file: string | null;
  file_name: string | null;
  /** True only when the file's contents are the problem, so renaming it away is the right offer. */
  can_set_aside: boolean;
  reason: string;
}
export const getStartupFailure = () => invoke<StartupFailure | null>("startup_failure");
/** Renames the unreadable file aside and returns its new path. */
export const setAsideUnreadableFile = () => invoke<string>("set_aside_unreadable_file");
export const restartApp = () => invoke<void>("restart_app");
