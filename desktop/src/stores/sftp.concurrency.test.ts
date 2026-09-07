import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import { useSftpStore } from "./sftp";
import type { Host } from "../types";

const host: Host = {
  id: "atlas",
  label: "Atlas",
  hostname: "atlas.example.test",
  port: 22,
  username: "deploy",
  auth: { password: { credential_key: "metadata-only" } },
  tags: [],
};

const otherHost: Host = {
  ...host,
  id: "orion",
  label: "Orion",
  hostname: "orion.example.test",
  username: "root",
};

const calls = (command: string) =>
  getInvokeMock().mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  setActivePinia(createPinia());
  setInvokeHandler("sftp_canonicalize", () => "/home/deploy");
  setInvokeHandler("sftp_list_dir", () => []);
});

describe("SFTP connection ownership", () => {
  it("shares one in-flight connection across duplicate submissions", async () => {
    let release!: () => void;
    setInvokeHandler("sftp_connect", () => new Promise<void>(resolve => { release = resolve; }));

    const sftp = useSftpStore();
    const first = sftp.connect(host, "SECRET", "deploy");
    const second = sftp.connect(host, "SHOULD_NOT_DISPATCH", "deploy");

    expect(second).toBe(first);
    await vi.waitFor(() => expect(calls("sftp_connect")).toHaveLength(1));
    expect(calls("sftp_connect")[0][1].password).toBe("SECRET");

    release();
    await first;

    expect(calls("sftp_canonicalize")).toHaveLength(1);
    expect(calls("sftp_list_dir")).toHaveLength(1);
    expect(sftp.sessionId).toBe(calls("sftp_connect")[0][1].sessionId);
    expect(sftp.error).toBeNull();
  });

  it("rejects a different host while the first immutable attempt is still pending", async () => {
    let release!: () => void;
    setInvokeHandler("sftp_connect", () => new Promise<void>(resolve => { release = resolve; }));

    const sftp = useSftpStore();
    const first = sftp.connect(host, "ATLAS_SECRET", "deploy");
    await vi.waitFor(() => expect(calls("sftp_connect")).toHaveLength(1));

    await expect(sftp.connect(otherHost, "ORION_SECRET", "root"))
      .rejects.toThrow("Another SFTP connection is still in progress");
    expect(calls("sftp_connect")).toHaveLength(1);
    expect(calls("sftp_connect")[0][1].host.id).toBe("atlas");
    expect(sftp.error).toContain("Another SFTP connection is still in progress");

    release();
    await first;
    expect(sftp.connectedHost?.id).toBe("atlas");
    expect(sftp.sessionId).toBe(calls("sftp_connect")[0][1].sessionId);
  });

  it("closes an obsolete late attempt without clearing a newer connection", async () => {
    let releaseFirst!: () => void;
    let connectionNumber = 0;
    setInvokeHandler("sftp_connect", () => {
      connectionNumber += 1;
      if (connectionNumber === 1) {
        return new Promise<void>(resolve => { releaseFirst = resolve; });
      }
      return undefined;
    });

    const sftp = useSftpStore();
    const stale = sftp.connect(host, "FIRST", "deploy");
    await vi.waitFor(() => expect(calls("sftp_connect")).toHaveLength(1));
    const staleId = calls("sftp_connect")[0][1].sessionId;

    await sftp.disconnect();
    const fresh = sftp.connect(host, "SECOND", "deploy");
    await fresh;
    const freshId = calls("sftp_connect")[1][1].sessionId;
    expect(sftp.sessionId).toBe(freshId);

    releaseFirst();
    await stale;

    expect(sftp.sessionId).toBe(freshId);
    expect(sftp.connectedHost?.id).toBe(host.id);
    expect(calls("sftp_close").map(([, args]) => args.sessionId)).toContain(staleId);
    expect(calls("sftp_close").map(([, args]) => args.sessionId)).not.toContain(freshId);
  });
});
