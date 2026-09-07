import type { AuthMethod, Host, Identity } from "../types";

/** A direct host is self-contained; only linked identities depend on the identity cache. */
export function sshIdentityReady(host: Host, identitiesLoaded: boolean): boolean {
  return !host.identity_id || identitiesLoaded;
}

/** Resolve the account/auth/key that this exact frontend configuration represents. */
export function effectiveSshIdentity(host: Host, identities: readonly Identity[]) {
  const identity = identities.find(item => item.id === host.identity_id);
  return {
    username: identity?.username ?? host.username,
    auth: identity?.auth ?? host.auth,
    keyId: identity ? identity.key_id : host.key_id,
  };
}

/**
 * Freeze the effective linked-identity configuration into an immutable transport
 * host. Clearing identity_id is intentional: the native command must authenticate
 * this exact reviewed username/auth/key snapshot rather than re-resolving a
 * same-id identity that may have changed after frontend preflight.
 */
export function resolvedSshHost(host: Host, identities: readonly Identity[]): Host {
  const effective = effectiveSshIdentity(host, identities);
  return {
    ...host,
    username: effective.username,
    auth: effective.auth,
    key_id: effective.keyId ?? null,
    identity_id: null,
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
