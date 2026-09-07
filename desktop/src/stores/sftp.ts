import { defineStore } from "pinia";
import { ref } from "vue";
import * as api from "../api";
import type { SftpEntry } from "../api";
import type { Host } from "../types";

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
    // The host passed here is already the frozen transport snapshot. Passwords
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
        await api.sftpConnect(id, host, password, expectedUsername);
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
        entries.value = initialEntries;
        selectedEntry.value = null;

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

  async function listDir(path: string) {
    if (!sessionId.value) return;
    error.value = null;
    loading.value = true;
    try {
      entries.value = await api.sftpListDir(sessionId.value, path);
      currentPath.value = path;
      selectedEntry.value = null;
    } catch (e) {
      error.value = String(e);
      throw e;
    } finally {
      loading.value = false;
    }
  }

  async function navigateToDir(name: string) {
    if (!sessionId.value) return;
    const newPath = joinPath(currentPath.value, name);
    await listDir(newPath);
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
    if (!sessionId.value) return;
    const newPath = joinPath(currentPath.value, name);
    await api.sftpCreateDir(sessionId.value, newPath);
    await refresh();
  }

  async function deleteEntry(entry: SftpEntry) {
    if (!sessionId.value) return;
    const fullPath = joinPath(currentPath.value, entry.name);
    if (entry.is_dir) {
      await api.sftpRemoveDir(sessionId.value, fullPath);
    } else {
      await api.sftpRemoveFile(sessionId.value, fullPath);
    }
    await refresh();
  }

  async function renameEntry(entry: SftpEntry, newName: string) {
    if (!sessionId.value) return;
    const oldPath = joinPath(currentPath.value, entry.name);
    const newPath = joinPath(currentPath.value, newName);
    await api.sftpRename(sessionId.value, oldPath, newPath);
    await refresh();
  }

  async function downloadFile(entry: SftpEntry, localPath: string) {
    if (!sessionId.value) throw new Error("Not connected");
    const fullPath = joinPath(currentPath.value, entry.name);
    error.value = null;
    loading.value = true;
    try {
      await api.sftpDownloadToLocal(sessionId.value, fullPath, localPath);
    } catch (e) {
      error.value = String(e);
      throw e;
    } finally {
      loading.value = false;
    }
  }

  async function uploadFile(
    name: string,
    localPath: string,
    overwrite = false,
  ) {
    if (!sessionId.value) throw new Error("Not connected");
    const fullPath = joinPath(currentPath.value, name);
    error.value = null;
    loading.value = true;
    try {
      await api.sftpUploadFromLocal(
        sessionId.value,
        localPath,
        fullPath,
        overwrite,
      );
      entries.value = await api.sftpListDir(sessionId.value, currentPath.value);
      selectedEntry.value = null;
    } catch (e) {
      error.value = String(e);
      throw e;
    } finally {
      loading.value = false;
    }
  }

  async function disconnect() {
    // Invalidate a native connection that has not published yet. Its continuation
    // observes the generation change and closes the late backend session itself.
    connectGeneration += 1;
    connectPending = null;
    connectPendingKey = null;
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

  function joinPath(base: string, name: string): string {
    if (base.endsWith("/")) return base + name;
    return base + "/" + name;
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
