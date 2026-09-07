import { describe, expect, it } from "vitest";
import { configuredSshEndpoint, effectiveSshIdentity, formatSshEndpoint, linkedSshIdentityMissing, passwordAuth, resolvedSshHost, sshConfigurationKey } from "./sshIdentity";
import type { Host, Identity } from "../types";

const host: Host = { id: "host", label: "Host", hostname: "server.example.test", port: 22,
  username: "deploy", auth: "agent", identity_id: "identity", key_id: "old", tags: [] };
const identity: Identity = { id: "identity", label: "Admin", username: "root",
  auth: { password: { credential_key: "metadata-only" } }, tags: [] };

describe("shared SSH identity presentation", () => {
  it("overrides all effective identity fields without mutating the saved host", () => {
    expect(effectiveSshIdentity(host, [identity])).toEqual({ username: "root", auth: identity.auth, keyId: undefined, missing: false });
    expect(configuredSshEndpoint(host, [identity])).toBe("root@server.example.test:22");
    expect(host.username).toBe("deploy");
  });
  it("pins the complete effective identity into a transport-only host", () => {
    let keyIdentity: Identity = { ...identity, auth: "publickey", key_id: "effective-key" };
    const transport = resolvedSshHost(host, [keyIdentity]);

    expect(transport).toEqual({
      ...host,
      username: "root",
      auth: "publickey",
      key_id: "effective-key",
      identity_id: null,
    });
    expect(host).toEqual({
      id: "host", label: "Host", hostname: "server.example.test", port: 22,
      username: "deploy", auth: "agent", identity_id: "identity", key_id: "old", tags: [],
    });

    // A later edit to the saved identity cannot mutate this attempt's snapshot.
    keyIdentity = { ...keyIdentity, auth: "agent", key_id: "replacement-key" };
    expect(transport.auth).toBe("publickey");
    expect(transport.key_id).toBe("effective-key");
  });
  it("preserves a broken identity reference so native authentication can fail closed", () => {
    expect(linkedSshIdentityMissing(host, [])).toBe(true);
    expect(configuredSshEndpoint(host, [])).toBe("Missing SSH identity · server.example.test:22");
    expect(resolvedSshHost(host, [])).toEqual(host);
    expect(effectiveSshIdentity(host, []).missing).toBe(true);
  });
  it("keeps direct hosts self-contained", () => {
    const direct = { ...host, identity_id: null };
    expect(linkedSshIdentityMissing(direct, [])).toBe(false);
    expect(configuredSshEndpoint(direct, [])).toBe("deploy@server.example.test:22");
    expect(resolvedSshHost(direct, []).identity_id).toBeNull();
  });
  it("recognizes effective password auth and formats IPv6 unambiguously", () => {
    expect(passwordAuth(identity.auth)).toBe(true);
    expect(passwordAuth("publickey")).toBe(false);
    expect(passwordAuth("agent")).toBe(false);
    expect(formatSshEndpoint({ username: "root", hostname: "::1", port: 22 })).toBe("root@[::1]:22");
  });
  it("detects changes to the account/auth/endpoint during a password prompt", () => {
    const key = sshConfigurationKey(host, [identity]);
    expect(sshConfigurationKey(host, [{ ...identity, username: "ops" }])).not.toBe(key);
    expect(sshConfigurationKey(host, [{ ...identity, auth: "agent" }])).not.toBe(key);
    expect(sshConfigurationKey({ ...host, hostname: "other.example.test" }, [identity])).not.toBe(key);
  });
});
