import { defineStore } from "pinia";
import { ref } from "vue";
import * as api from "../api";
import type { Workspace } from "../types";

export const useWorkspacesStore = defineStore("workspaces", () => {
  const workspaces = ref<Workspace[]>([]);
  const activeWorkspaceId = ref<string | null>(null);
  // Lets the view tell "none saved" apart from "not read yet" and "could not
  // be read", which all leave `workspaces` empty.
  const loaded = ref(false);
  const loading = ref(false);
  const loadError = ref("");

  async function load() {
    loading.value = true;
    loadError.value = "";
    try {
      workspaces.value = await api.listWorkspaces();
      loaded.value = true;
    } catch (e) {
      loadError.value = String(e);
      throw e;
    } finally {
      loading.value = false;
    }
  }

  async function createWorkspace(name: string) {
    const ws = await api.createWorkspace(name);
    workspaces.value.push(ws);
    return ws;
  }

  async function saveWorkspace(ws: Workspace) {
    const saved = await api.saveWorkspace(ws);
    const idx = workspaces.value.findIndex((w) => w.id === saved.id);
    if (idx >= 0) workspaces.value[idx] = saved;
    return saved;
  }

  async function deleteWorkspace(id: string) {
    await api.deleteWorkspace(id);
    workspaces.value = workspaces.value.filter((w) => w.id !== id);
    if (activeWorkspaceId.value === id) activeWorkspaceId.value = null;
  }

  async function setActive(id: string) {
    await api.setActiveWorkspace(id);
    activeWorkspaceId.value = id;
  }

  return {
    workspaces,
    activeWorkspaceId,
    loaded,
    loading,
    loadError,
    load,
    createWorkspace,
    saveWorkspace,
    deleteWorkspace,
    setActive,
  };
});
