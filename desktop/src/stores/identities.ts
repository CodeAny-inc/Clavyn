import { defineStore } from "pinia";
import { ref, computed } from "vue";
import * as api from "../api";
import type { Identity } from "../types";
import { useHostsStore } from "./hosts";

export const useIdentitiesStore = defineStore("identities", () => {
  const identities = ref<Identity[]>([]);
  const loaded = ref(false);
  const loading = ref(false);
  const loadError = ref("");
  let pendingLoad: Promise<void> | null = null;
  // Bumped by every local change. A load that started before the latest change
  // would replace the list with data that predates it, so its result is dropped.
  let identitiesRevision = 0;
  const searchQuery = ref("");
  const selectedGroupId = ref<string | null>(null);

  const filteredIdentities = computed(() => {
    let result = identities.value;
    if (selectedGroupId.value) {
      result = result.filter((i) => i.group_id === selectedGroupId.value);
    }
    if (searchQuery.value.trim()) {
      const q = searchQuery.value.toLowerCase();
      result = result.filter(
        (i) =>
          i.label.toLowerCase().includes(q) ||
          i.username.toLowerCase().includes(q) ||
          i.tags.some((t) => t.toLowerCase().includes(q)),
      );
    }
    return result;
  });

  function load(): Promise<void> {
    if (pendingLoad) return pendingLoad;
    loaded.value = false;
    loading.value = true;
    loadError.value = "";
    const startedAt = identitiesRevision;
    pendingLoad = api.listIdentities().then(items => {
      if (!Array.isArray(items)) throw new Error("Invalid identity list response");
      if (identitiesRevision !== startedAt) return;
      identities.value = items;
      loaded.value = true;
    }).catch(() => {
      // Legacy onMounted callers fire-and-forget load(). Surface failure in state;
      // connection preflight uses ensureLoaded(), which explicitly rejects it.
      loadError.value = "Could not load SSH identities. Retry before connecting.";
    }).finally(() => {
      loading.value = false;
      pendingLoad = null;
    });
    return pendingLoad;
  }

  async function ensureLoaded() {
    if (!loaded.value) await load();
    if (!loaded.value) throw new Error(loadError.value || "SSH identities are not loaded.");
  }

  async function addIdentity(identity: Identity) {
    const saved = await api.addIdentity(identity);
    identitiesRevision += 1;
    identities.value.push(saved);
    return saved;
  }

  async function updateIdentity(identity: Identity) {
    const saved = await api.updateIdentity(identity);
    identitiesRevision += 1;
    const idx = identities.value.findIndex((i) => i.id === saved.id);
    if (idx >= 0) identities.value[idx] = saved;
    return saved;
  }

  async function deleteIdentity(id: string) {
    await api.deleteIdentity(id);
    identitiesRevision += 1;
    identities.value = identities.value.filter((i) => i.id !== id);
    useHostsStore().unlinkIdentity(id);
  }

  // Called when a group is deleted: the backend ungroups its identities too.
  function ungroup(groupId: string) {
    identitiesRevision += 1;
    identities.value.forEach((i) => {
      if (i.group_id === groupId) i.group_id = null;
    });
  }

  return {
    identities, loaded, loading, loadError, ensureLoaded,
    searchQuery,
    selectedGroupId,
    filteredIdentities,
    load,
    addIdentity,
    updateIdentity,
    deleteIdentity,
    ungroup,
  };
});
