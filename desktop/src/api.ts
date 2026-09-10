// Tauri command wrappers — thin typed layer over `invoke`.
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Host,
  HostGroup,
  Identity,
  KeyMeta,
  KnownHostEntry,
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
export const unlockVault = (passphrase: string) => invoke<void>("secure_unlock_vault", { passphrase });
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
export const listWorkspaces = () => invoke<Workspace[]>("list_workspaces");
export const createWorkspace = (name: string) => invoke<Workspace>("create_workspace", { name });
export const saveWorkspace = (workspace: Workspace) => invoke<Workspace>("save_workspace", { workspace });
export const deleteWorkspace = (id: string) => invoke<void>("delete_workspace", { id });
export const setActiveWorkspace = (id: string) => invoke<void>("set_active_workspace", { id });
export const readKeyFile = (path: string) => invoke<string>("read_key_file", { path });

/**
 * Sink a session's terminal output arrives on. Bytes travel as a binary IPC
 * payload, so they never pass through JSON, and each session has its own sink
 * instead of every pane receiving every session's output.
 */
export type SessionOutput = Channel<ArrayBuffer>;

export function sessionOutput(onData: (bytes: Uint8Array) => void): SessionOutput {
  const channel: SessionOutput = new Channel();
  channel.onmessage = (message) => onData(new Uint8Array(message));
  return channel;
}

export interface SshConnectionInfo { username: string; hostname: string; port: number }
export const connectSsh = (sessionId: string, host: Host, password: string | null, cols: number, rows: number, onOutput: SessionOutput, expectedUsername?: string) =>
  invoke<SshConnectionInfo>("connect_ssh", { sessionId, host, password, cols, rows, expectedUsername, onOutput });
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
  host: Host,
  password: string | null,
  expectedUsername?: string,
) => invoke<void>("sftp_connect", { sessionId, host, password, expectedUsername });
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
export const sftpDownloadToLocal = (
  sessionId: string,
  remotePath: string,
  localPath: string,
) => invoke<void>("sftp_download_to_local", { sessionId, remotePath, localPath });
export const sftpUploadFromLocal = (
  sessionId: string,
  localPath: string,
  remotePath: string,
  overwrite: boolean,
) => invoke<void>("sftp_upload_from_local", { sessionId, localPath, remotePath, overwrite });

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
export function onUpdateAvailable(cb: (e: UpdateInfo) => void): Promise<UnlistenFn> {
  return listen<UpdateInfo>("update-available", (event) => cb(event.payload));
}
export function onUpdateProgress(cb: (e: UpdateProgress) => void): Promise<UnlistenFn> {
  return listen<UpdateProgress>("update-progress", (event) => cb(event.payload));
}
export function onUpdateExtracting(cb: () => void): Promise<UnlistenFn> {
  return listen("update-extracting", () => cb());
}
