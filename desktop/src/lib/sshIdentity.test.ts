import { describe, expect, it } from "vitest";
import { configuredSshEndpoint, effectiveSshIdentity, formatSshEndpoint, passwordAuth, sshConfigurationKey } from "./sshIdentity";
import type { Host, Identity } from "../types";

const host: Host = { id: "host", label: "Host", hostname: "server.example.test", port: 22,
  username: "deploy", auth: "agent", identity_id: "identity", key_id: "old", tags: [] };
const identity: Identity = { id: "identity", label: "Admin", username: "root",
  auth: { password: { credential_key: "metadata-only" } }, tags: [] };

describe("shared SSH identity presentation", () => {
  it("overrides all effective identity fields without mutating the saved host", () => {
    expect(effectiveSshIdentity(host, [identity])).toEqual({ username: "root", auth: identity.auth, keyId: undefined });
    expect(configuredSshEndpoint(host, [identity])).toBe("root@server.example.test:22");
    expect(host.username).toBe("deploy");
  });
  it("falls back only when a loaded identity list has no matching entry", () => {
    expect(configuredSshEndpoint(host, [])).toBe("deploy@server.example.test:22");
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
