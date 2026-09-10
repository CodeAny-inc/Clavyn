// Types matching the Rust core structs.

export type Uuid = string;

export interface Host {
  id: Uuid;
  label: string;
  hostname: string;
  port: number;
  username: string;
  group_id?: Uuid | null;
  key_id?: Uuid | null;
  auth: AuthMethod;
  tags: string[];
  startup_command?: string | null;
  proxy_command?: string | null;
  jump_host_id?: Uuid | null;
  identity_id?: Uuid | null;
}

export type AuthMethod =
  | { password: { credential_key: string } }
  | "publickey"
  | "agent";

export interface HostGroup {
  id: Uuid;
  name: string;
  color?: string | null;
}

export interface Identity {
  id: Uuid;
  label: string;
  username: string;
  auth: AuthMethod;
  key_id?: Uuid | null;
  tags: string[];
  group_id?: Uuid | null;
}

export interface KeyMeta {
  id: Uuid;
  label: string;
  key_type: "ed25519" | "rsa" | "ecdsa";
  fingerprint: string;
  public_key_base64: string;
}

export interface KnownHostEntry {
  host: string;
  key_type: string;
  fingerprint: string;
}

export interface Workspace {
  id: Uuid;
  name: string;
  tabs: TabLayout[];
  icon?: string | null;
  description?: string | null;
  color?: string | null;
  host_ids?: Uuid[];
  auto_connect?: boolean;
}

export interface TabLayout {
  id: Uuid;
  title: string;
  layout: PaneLayout;
}

export type PaneLayout =
  | { type: "pane"; host_id?: Uuid | null; terminal_type: string }
  | {
      type: "split";
      direction: "horizontal" | "vertical";
      ratio: number;
      first: PaneLayout;
      second: PaneLayout;
    };

export interface SessionClosedEvent {
  session_id: string;
  reason: string;
}
