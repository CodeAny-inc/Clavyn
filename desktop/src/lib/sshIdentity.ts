import type { AuthMethod, Host, Identity } from "../types";

/** A direct host is self-contained; only linked identities depend on the identity cache. */
export function sshIdentityReady(host: Host, identitiesLoaded: boolean): boolean {
  return !host.identity_id || identitiesLoaded;
}

/** Presentation/preflight only. Rust still resolves and authenticates the identity. */
export function effectiveSshIdentity(host: Host, identities: readonly Identity[]) {
  const identity = identities.find(item => item.id === host.identity_id);
  return {
    username: identity?.username ?? host.username,
    auth: identity?.auth ?? host.auth,
    keyId: identity ? identity.key_id : host.key_id,
  };
}

export function formatSshEndpoint(endpoint: { username: string; hostname: string; port: number }) {
  const hostname = endpoint.hostname.includes(":") && !endpoint.hostname.startsWith("[")
    ? `[${endpoint.hostname}]` : endpoint.hostname;
  return `${endpoint.username}@${hostname}:${endpoint.port}`;
}

export function configuredSshEndpoint(host: Host, identities: readonly Identity[]) {
  return formatSshEndpoint({ ...host, username: effectiveSshIdentity(host, identities).username });
}

export function passwordAuth(auth: AuthMethod): boolean {
  return typeof auth === "object" && auth !== null && "password" in auth;
}

/** Detect edits while a user is entering credentials; never reuse them for new settings. */
export function sshConfigurationKey(host: Host, identities: readonly Identity[]) {
  return JSON.stringify([host, effectiveSshIdentity(host, identities)]);
}
