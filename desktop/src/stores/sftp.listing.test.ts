import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import { useSftpStore, type SftpEntryRef } from "./sftp";
import type { SftpEntry } from "../api";
import type { Host } from "../types";

const host: Host = {
  id: "atlas",
  label: "Atlas",
  hostname: "atlas.example.test",
  port: 22,
  username: "u",
  auth: "agent",
  tags: [],
};

const entry = (name: string, is_dir = false): SftpEntry => ({
  name, long_name: name, is_dir, is_file: !is_dir, is_symlink: false,
  size: 1, modified: null, permissions: null,
});

const notes = entry("notes.txt");
const docs = entry("docs", true);

const calls = (command: string) =>
  getInvokeMock().mock.calls.filter(([name]) => name === command);

// Listings answer immediately from `listings` unless a test holds a path, in
// which case that request stays pending until the test releases it.
let listings: Record<string, SftpEntry[]>;
let held: Map<string, Array<(entries: SftpEntry[]) => void>>;

function hold(path: string) {
  held.set(path, []);
}

function release(path: string, entries = listings[path] ?? []) {
  const pending = held.get(path)?.shift();
  if (!pending) throw new Error(`no pending listing for ${path}`);
  pending(entries);
}

async function connect() {
  const sftp = useSftpStore();
  await sftp.connect(host, null, "u");
  return sftp;
}

const refOf = (sftp: ReturnType<typeof useSftpStore>, target: SftpEntry): SftpEntryRef =>
  ({ sessionId: sftp.sessionId!, dir: sftp.currentPath, entry: target });

beforeEach(() => {
  setActivePinia(createPinia());
  listings = {
    "/home/u": [docs, notes],
    "/home/u/docs": [notes],
    "/srv": [entry("srv.txt")],
  };
  held = new Map();
  setInvokeHandler("sftp_connect", () => undefined);
  setInvokeHandler("sftp_canonicalize", () => "/home/u");
  setInvokeHandler("sftp_list_dir", ({ path }: { path: string }) => {
    const queue = held.get(path);
    if (queue) return new Promise<SftpEntry[]>(resolve => queue.push(resolve));
    return listings[path] ?? [];
  });
  setInvokeHandler("sftp_remove_file", () => undefined);
  setInvokeHandler("sftp_rename", () => undefined);
  setInvokeHandler("sftp_download_to_local", () => true);
});

describe("per-entry actions keep the directory the entry was listed from", () => {
  it("deletes the captured entry in its own directory after a navigation lands", async () => {
    const sftp = await connect();
    const captured = refOf(sftp, notes);

    hold("/home/u/docs");
    const navigation = sftp.navigateToDir(refOf(sftp, docs));
    release("/home/u/docs");
    await navigation;
    expect(sftp.currentPath).toBe("/home/u/docs");

    await sftp.deleteEntry(captured);

    expect(calls("sftp_remove_file").map(([, args]) => args.path)).toEqual(["/home/u/notes.txt"]);
    // The browser now shows docs, so refreshing /home/u would pull it back.
    expect(sftp.currentPath).toBe("/home/u/docs");
  });

  it("renames and downloads in the captured directory", async () => {
    const sftp = await connect();
    const captured = refOf(sftp, notes);
    await sftp.navigateToDir(refOf(sftp, docs));

    await sftp.renameEntry(captured, "todo.txt");
    await sftp.downloadFile(captured);

    expect(calls("sftp_rename")[0][1]).toMatchObject({
      oldPath: "/home/u/notes.txt",
      newPath: "/home/u/todo.txt",
    });
    expect(calls("sftp_download_to_local")[0][1].remotePath).toBe("/home/u/notes.txt");
  });

  it("refuses an entry captured from a session that is no longer open", async () => {
    const sftp = await connect();
    const captured = refOf(sftp, notes);
    await sftp.disconnect();
    await sftp.connect(host, null, "u");

    await expect(sftp.deleteEntry(captured)).rejects.toThrow("no longer open");
    expect(calls("sftp_remove_file")).toHaveLength(0);
  });

  it("reports an action on a closed session through the store error", async () => {
    const sftp = await connect();
    const captured = refOf(sftp, notes);
    await sftp.disconnect();

    await expect(sftp.deleteEntry(captured)).rejects.toThrow("Not connected");
    expect(sftp.error).toBe("Not connected");
    expect(calls("sftp_remove_file")).toHaveLength(0);
  });
});

describe("only the latest listing publishes", () => {
  it("ignores an older listing that lands after a newer one", async () => {
    const sftp = await connect();
    hold("/home/u/docs");
    hold("/srv");

    const first = sftp.listDir("/home/u/docs");
    const second = sftp.listDir("/srv");
    release("/srv");
    await second;
    expect(sftp.currentPath).toBe("/srv");
    expect(sftp.loading).toBe(false);

    release("/home/u/docs");
    await first;
    expect(sftp.currentPath).toBe("/srv");
    expect(sftp.entries.map(e => e.name)).toEqual(["srv.txt"]);
    expect(sftp.loading).toBe(false);
  });

  it("keeps loading until the latest listing lands, even if an older one finishes first", async () => {
    const sftp = await connect();
    hold("/home/u/docs");
    hold("/srv");

    const first = sftp.listDir("/home/u/docs");
    const second = sftp.listDir("/srv");
    release("/home/u/docs");
    await first;
    expect(sftp.loading).toBe(true);
    expect(sftp.currentPath).toBe("/home/u");
    expect(sftp.entries.map(e => e.name)).toEqual(["docs", "notes.txt"]);

    release("/srv");
    await second;
    expect(sftp.currentPath).toBe("/srv");
    expect(sftp.entries.map(e => e.name)).toEqual(["srv.txt"]);
    expect(sftp.loading).toBe(false);
  });

  it("drops a listing that belongs to a session closed while it was in flight", async () => {
    const sftp = await connect();
    hold("/srv");
    const stale = sftp.listDir("/srv");

    await sftp.disconnect();
    await sftp.connect(host, null, "u");
    expect(sftp.loading).toBe(false);

    release("/srv");
    await stale;
    expect(sftp.currentPath).toBe("/home/u");
    expect(sftp.entries.map(e => e.name)).toEqual(["docs", "notes.txt"]);
    expect(sftp.loading).toBe(false);
  });

  it("re-lists the browsed directory when a navigation away from it fails after a change", async () => {
    const sftp = await connect();
    let failNavigation!: (reason: Error) => void;
    setInvokeHandler("sftp_list_dir", ({ path }: { path: string }) => {
      if (path === "/home/u/docs") {
        return new Promise<SftpEntry[]>((_, reject) => { failNavigation = reject; });
      }
      return listings[path] ?? [];
    });

    const navigation = sftp.listDir("/home/u/docs");
    listings["/home/u"] = [docs];
    await sftp.deleteEntry(refOf(sftp, notes));
    const listingsBefore = calls("sftp_list_dir").length;

    failNavigation(new Error("permission denied"));
    await expect(navigation).rejects.toThrow("permission denied");
    await vi.waitFor(() => expect(sftp.entries.map(e => e.name)).toEqual(["docs"]));

    expect(calls("sftp_list_dir").slice(listingsBefore).map(([, args]) => args.path)).toEqual(["/home/u"]);
    expect(sftp.currentPath).toBe("/home/u");
    expect(sftp.error).toContain("permission denied");
    expect(sftp.loading).toBe(false);
  });

  it("does not let an upload's refresh overwrite a navigation that happened meanwhile", async () => {
    let finishUpload!: () => void;
    setInvokeHandler("sftp_upload_from_local", () => new Promise<void>(resolve => { finishUpload = resolve; }));
    const sftp = await connect();

    const upload = sftp.uploadFile("new.txt", "token");
    await vi.waitFor(() => expect(calls("sftp_upload_from_local")).toHaveLength(1));
    await sftp.navigateToDir(refOf(sftp, docs));
    const listingsBefore = calls("sftp_list_dir").length;

    finishUpload();
    await upload;

    expect(calls("sftp_upload_from_local")[0][1].remotePath).toBe("/home/u/new.txt");
    expect(calls("sftp_list_dir")).toHaveLength(listingsBefore);
    expect(sftp.currentPath).toBe("/home/u/docs");
    expect(sftp.entries.map(e => e.name)).toEqual(["notes.txt"]);
    expect(sftp.loading).toBe(false);
  });
});
