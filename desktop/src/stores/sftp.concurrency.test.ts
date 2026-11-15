import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import { useSftpStore } from "./sftp";
import type { SftpEntry } from "../api";
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

    // Pinia wraps action return values, so callers do not necessarily observe
    // Object.is-identical Promise instances even when the store shares the same
    // underlying task. The ownership invariant is one backend dispatch and both
    // callers settling from that single attempt.
    await vi.waitFor(() => expect(calls("sftp_connect")).toHaveLength(1));
    expect(calls("sftp_connect")[0][1].password).toBe("SECRET");

    release();
    await Promise.all([first, second]);

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
    expect(calls("sftp_connect")[0][1].hostId).toBe("atlas");
    expect(sftp.error).toContain("Another SFTP connection is still in progress");

    release();
    await first;
    expect(sftp.connectedHost?.id).toBe("atlas");
    expect(sftp.sessionId).toBe(calls("sftp_connect")[0][1].sessionId);
    expect(sftp.error).toBeNull();
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

describe("SFTP directory listings", () => {
  const entry = (name: string): SftpEntry => ({
    name,
    long_name: name,
    is_dir: true,
    is_file: false,
    is_symlink: false,
    size: 0,
    modified: null,
    permissions: null,
  });

  // Each listing waits until the test answers it, so the order in which
  // listings answer is the test's to choose.
  async function connectedWithHeldListings() {
    const pending = new Map<string, { resolve: (value: SftpEntry[]) => void; reject: (e: Error) => void }>();
    setInvokeHandler("sftp_connect", () => undefined);
    const sftp = useSftpStore();
    await sftp.connect(host, "SECRET", "deploy");
    setInvokeHandler("sftp_list_dir", ({ path }: { path: string }) =>
      new Promise<SftpEntry[]>((resolve, reject) => { pending.set(path, { resolve, reject }); }));
    return { sftp, pending };
  }

  it("keeps the newer directory when an older listing answers last", async () => {
    const { sftp, pending } = await connectedWithHeldListings();
    const older = sftp.listDir("/var/log");
    const newer = sftp.listDir("/etc");
    await vi.waitFor(() => expect(pending.size).toBe(2));

    pending.get("/etc")!.resolve([entry("nginx")]);
    await newer;
    pending.get("/var/log")!.resolve([entry("journal")]);
    await older;

    expect(sftp.currentPath).toBe("/etc");
    expect(sftp.entries.map(e => e.name)).toEqual(["nginx"]);
    expect(sftp.loading).toBe(false);
  });

  it("stays loading until the latest listing answers", async () => {
    const { sftp, pending } = await connectedWithHeldListings();
    const older = sftp.listDir("/var/log");
    const newer = sftp.listDir("/etc");
    await vi.waitFor(() => expect(pending.size).toBe(2));

    pending.get("/var/log")!.resolve([entry("journal")]);
    await older;
    expect(sftp.loading).toBe(true);
    expect(sftp.currentPath).toBe("/home/deploy");

    pending.get("/etc")!.resolve([entry("nginx")]);
    await newer;
    expect(sftp.loading).toBe(false);
    expect(sftp.currentPath).toBe("/etc");
  });

  it("ignores the failure of a listing the user has moved on from", async () => {
    const { sftp, pending } = await connectedWithHeldListings();
    const older = sftp.listDir("/root");
    const newer = sftp.listDir("/etc");
    await vi.waitFor(() => expect(pending.size).toBe(2));

    pending.get("/etc")!.resolve([entry("nginx")]);
    await newer;
    pending.get("/root")!.reject(new Error("permission denied"));
    await expect(older).resolves.toBeUndefined();

    expect(sftp.error).toBeNull();
    expect(sftp.currentPath).toBe("/etc");
  });

  it("publishes nothing from a listing that answers after a disconnect", async () => {
    const { sftp, pending } = await connectedWithHeldListings();
    const listing = sftp.listDir("/etc");
    await vi.waitFor(() => expect(pending.size).toBe(1));

    await sftp.disconnect();
    pending.get("/etc")!.resolve([entry("nginx")]);
    await listing;

    expect(sftp.sessionId).toBeNull();
    expect(sftp.entries).toEqual([]);
    expect(sftp.currentPath).toBe("/");
  });
});
