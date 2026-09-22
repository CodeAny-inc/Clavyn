import type { AuthMethod, Host, Identity } from "../types";

/** A direct host is self-contained; only linked identities depend on the identity cache. */
export function sshIdentityReady(host: Host, identitiesLoaded: boolean): boolean {
  return !host.identity_id || identitiesLoaded;
}

/** A loaded cache with no referenced identity is a broken linked configuration, not a direct host. */
export function linkedSshIdentityMissing(host: Host, identities: readonly Identity[]): boolean {
  return !!host.identity_id && !identities.some(item => item.id === host.identity_id);
}

/** Resolve the account/auth/key that this exact frontend configuration represents. */
export function effectiveSshIdentity(host: Host, identities: readonly Identity[]) {
  const identity = identities.find(item => item.id === host.identity_id);
  return {
    username: identity?.username ?? host.username,
    auth: identity?.auth ?? host.auth,
    keyId: identity ? identity.key_id : host.key_id,
    missing: linkedSshIdentityMissing(host, identities),
  };
}

export function formatSshEndpoint(endpoint: { username: string; hostname: string; port: number }) {
  const hostname = endpoint.hostname.includes(":") && !endpoint.hostname.startsWith("[")
    ? `[${endpoint.hostname}]` : endpoint.hostname;
  return `${endpoint.username}@${hostname}:${endpoint.port}`;
}

export function configuredSshEndpoint(host: Host, identities: readonly Identity[]) {
  const effective = effectiveSshIdentity(host, identities);
  if (effective.missing) return `Missing SSH identity · ${host.hostname}:${host.port}`;
  return formatSshEndpoint({ ...host, username: effective.username });
}

export function passwordAuth(auth: AuthMethod): boolean {
  return typeof auth === "object" && auth !== null && "password" in auth;
}

/** Detect edits while a user is entering credentials; never reuse them for new settings. */
export function sshConfigurationKey(host: Host, identities: readonly Identity[]) {
  return JSON.stringify([host, effectiveSshIdentity(host, identities)]);
}
