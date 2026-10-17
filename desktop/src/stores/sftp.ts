import { defineStore } from "pinia";
import { ref } from "vue";
import * as api from "../api";
import type { SftpEntry } from "../api";
import type { Host } from "../types";

/** An entry together with the directory and session it was listed from. */
export interface SftpEntryRef {
  sessionId: string;
  dir: string;
  entry: SftpEntry;
}

export function joinPath(base: string, name: string): string {
  if (base.endsWith("/")) return base + name;
  return base + "/" + name;
}

export const useSftpStore = defineStore("sftp", () => {
  const sessionId = ref<string | null>(null);
  const connectedHost = ref<Host | null>(null);
  const currentPath = ref("/");
  const entries = ref<SftpEntry[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const selectedEntry = ref<SftpEntry | null>(null);

  // Presentation controls disable Connect while loading, but keyboard submission
  // can still call connect() again. Keep the invariant in the store so every UI
  // surface shares one in-flight establishment attempt for the SAME immutable
  // connection configuration only.
  let connectPending: Promise<void> | null = null;
  let connectPendingKey: string | null = null;
  let connectGeneration = 0;

  function clearPublishedConnection() {
    sessionId.value = null;
    connectedHost.value = null;
    entries.value = [];
    currentPath.value = "/";
    selectedEntry.value = null;
  }

  function connectionRequestKey(host: Host, expectedUsername?: string) {
    // The host is the saved host plus the reviewed username; the backend
    // resolves its identity and connection details from the store. Passwords
    // are intentionally excluded so duplicate submit/click events share one task.
    return JSON.stringify([host, expectedUsername ?? null]);
  }

  function connect(
    host: Host,
    password: string | null = null,
    expectedUsername?: string,
  ): Promise<void> {
    const requestKey = connectionRequestKey(host, expectedUsername);
    if (connectPending) {
      if (connectPendingKey === requestKey) return connectPending;
      const message = "Another SFTP connection is still in progress. Wait for it to finish or disconnect before connecting a different host.";
      error.value = message;
      return Promise.reject(new Error(message));
    }

    const generation = ++connectGeneration;
    const id = crypto.randomUUID();
    const task = (async () => {
      error.value = null;
      loading.value = true;
      let connected = false;

      try {
        await api.sftpConnect(id, host.id, password, expectedUsername);
        connected = true;

        // Disconnect/unmount can invalidate an attempt while native auth is in
        // flight. Never let that late attempt publish a session into fresh UI state.
        if (generation !== connectGeneration) {
          await api.sftpClose(id).catch(() => {});
          return;
        }

        // Complete initial navigation before publishing the session into store
        // state. If any bootstrap step fails we can close the partially-created
        // backend session and avoid leaking it.
        const home = await api.sftpCanonicalize(id, "~");
        if (generation !== connectGeneration) {
          await api.sftpClose(id).catch(() => {});
          return;
        }
        const initialEntries = await api.sftpListDir(id, home);
        if (generation !== connectGeneration) {
          await api.sftpClose(id).catch(() => {});
          return;
        }

        const previous = sessionId.value;
        sessionId.value = id;
        connectedHost.value = host;
        currentPath.value = home;
        destinationPath = home;
        entries.value = initialEntries;
        selectedEntry.value = null;
        // A rejected attempt to switch hosts while this task was pending may
        // have surfaced a transient ownership error. Once the owning attempt
        // succeeds, its published state is authoritative again.
        error.value = null;

        // Defensive replacement support: UI normally connects only while
        // disconnected, but never leak an older published backend session.
        if (previous && previous !== id) {
          await api.sftpClose(previous).catch(() => {});
        }
      } catch (e) {
        if (connected) {
          await api.sftpClose(id).catch(() => {});
        }
        if (generation !== connectGeneration) return;
        clearPublishedConnection();
        error.value = String(e);
        throw e;
      } finally {
        if (generation === connectGeneration) loading.value = false;
      }
    })();

    connectPending = task;
    connectPendingKey = requestKey;
    task.then(
      () => {
        if (connectPending === task) {
          connectPending = null;
          connectPendingKey = null;
        }
      },
      () => {
        if (connectPending === task) {
          connectPending = null;
          connectPendingKey = null;
        }
      },
    );
    return task;
  }

  // Listings can finish out of order, so only the most recent request may
  // publish its entries or end the spinner. `destinationPath` is the directory
  // the browser shows, or is loading to show once that request lands.
  let listGeneration = 0;
  let listPending = false;
  let destinationPath = "/";
  let transfers = 0;

  function settleLoading() {
    loading.value = listPending || transfers > 0;
  }

  async function listDir(path: string) {
    const id = sessionId.value;
    if (!id) return;
    const request = ++listGeneration;
    listPending = true;
    destinationPath = path;
    error.value = null;
    loading.value = true;
    try {
      const listed = await api.sftpListDir(id, path);
      if (request !== listGeneration || id !== sessionId.value) return;
      entries.value = listed;
      currentPath.value = path;
      selectedEntry.value = null;
    } catch (e) {
      if (request !== listGeneration || id !== sessionId.value) return;
      destinationPath = currentPath.value;
      error.value = String(e);
      throw e;
    } finally {
      if (request === listGeneration) {
        listPending = false;
        settleLoading();
      }
    }
  }

  // A change re-lists its directory only while the browser is still there;
  // once the user has navigated elsewhere the refresh would undo that.
  async function refreshAfterChange(id: string, dir: string) {
    if (sessionId.value === id && destinationPath === dir) await listDir(dir);
  }

  // Per-entry actions resolve against the directory and session the entry was
  // listed from, never against whatever the browser shows when the action
  // runs, so a navigation that lands in between cannot retarget them.
  function requireTarget(target: SftpEntryRef): string {
    if (!sessionId.value) throw new Error("Not connected");
    if (target.sessionId !== sessionId.value) {
      const message = "This entry belongs to an SFTP session that is no longer open.";
      error.value = message;
      throw new Error(message);
    }
    return target.sessionId;
  }

  async function navigateToDir(target: SftpEntryRef) {
    requireTarget(target);
    await listDir(joinPath(target.dir, target.entry.name));
  }

  async function goUp() {
    if (!sessionId.value) return;
    const parts = currentPath.value.split("/").filter(Boolean);
    parts.pop();
    const parent = parts.length === 0 ? "/" : "/" + parts.join("/");
    await listDir(parent);
  }

  async function refresh() {
    await listDir(currentPath.value);
  }

  async function createDir(name: string) {
    const id = sessionId.value;
    if (!id) return;
    const dir = currentPath.value;
    await api.sftpCreateDir(id, joinPath(dir, name));
    await refreshAfterChange(id, dir);
  }

  async function deleteEntry(target: SftpEntryRef) {
    const id = requireTarget(target);
    const fullPath = joinPath(target.dir, target.entry.name);
    if (target.entry.is_dir) {
      await api.sftpRemoveDir(id, fullPath);
    } else {
      await api.sftpRemoveFile(id, fullPath);
    }
    await refreshAfterChange(id, target.dir);
  }

  async function renameEntry(target: SftpEntryRef, newName: string) {
    const id = requireTarget(target);
    const oldPath = joinPath(target.dir, target.entry.name);
    const newPath = joinPath(target.dir, newName);
    await api.sftpRename(id, oldPath, newPath);
    await refreshAfterChange(id, target.dir);
  }

  async function downloadFile(target: SftpEntryRef) {
    const id = requireTarget(target);
    const fullPath = joinPath(target.dir, target.entry.name);
    error.value = null;
    transfers += 1;
    loading.value = true;
    try {
      await api.sftpDownloadToLocal(id, fullPath);
    } catch (e) {
      error.value = String(e);
      throw e;
    } finally {
      transfers -= 1;
      if (id === sessionId.value) settleLoading();
    }
  }

  async function uploadFile(
    name: string,
    uploadToken: string,
    overwrite = false,
  ) {
    const id = sessionId.value;
    if (!id) throw new Error("Not connected");
    const dir = currentPath.value;
    error.value = null;
    transfers += 1;
    loading.value = true;
    try {
      await api.sftpUploadFromLocal(
        id,
        uploadToken,
        joinPath(dir, name),
        overwrite,
      );
    } catch (e) {
      error.value = String(e);
      throw e;
    } finally {
      transfers -= 1;
      if (id === sessionId.value) settleLoading();
    }
    await refreshAfterChange(id, dir);
  }

  async function disconnect() {
    // Invalidate a native connection that has not published yet. Its continuation
    // observes the generation change and closes the late backend session itself.
    connectGeneration += 1;
    connectPending = null;
    connectPendingKey = null;
    listGeneration += 1;
    listPending = false;
    loading.value = false;
    const id = sessionId.value;
    try {
      if (id) {
        await api.sftpClose(id);
      }
    } finally {
      clearPublishedConnection();
      error.value = null;
    }
  }

  return {
    sessionId,
    connectedHost,
    currentPath,
    entries,
    loading,
    error,
    selectedEntry,
    connect,
    listDir,
    navigateToDir,
    goUp,
    refresh,
    createDir,
    deleteEntry,
    renameEntry,
    downloadFile,
    uploadFile,
    disconnect,
  };
});
