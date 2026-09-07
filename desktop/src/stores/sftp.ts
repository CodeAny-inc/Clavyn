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

  async function connect(
    host: Host,
    password: string | null = null,
    expectedUsername?: string,
  ) {
    error.value = null;
    loading.value = true;
    const id = crypto.randomUUID();
    let connected = false;

    try {
      await api.sftpConnect(id, host, password, expectedUsername);
      connected = true;

      // Complete initial navigation before publishing the session into store
      // state. If any bootstrap step fails we can close the partially-created
      // backend session and avoid leaking it.
      const home = await api.sftpCanonicalize(id, "~");
      const initialEntries = await api.sftpListDir(id, home);

      sessionId.value = id;
      connectedHost.value = host;
      currentPath.value = home;
      entries.value = initialEntries;
      selectedEntry.value = null;
    } catch (e) {
      if (connected) {
        await api.sftpClose(id).catch(() => {});
      }
      sessionId.value = null;
      connectedHost.value = null;
      entries.value = [];
      currentPath.value = "/";
      selectedEntry.value = null;
      error.value = String(e);
      throw e;
    } finally {
      loading.value = false;
    }
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
    const id = sessionId.value;
    try {
      if (id) {
        await api.sftpClose(id);
      }
    } finally {
      sessionId.value = null;
      connectedHost.value = null;
      entries.value = [];
      currentPath.value = "/";
      selectedEntry.value = null;
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
