import { defineStore } from "pinia";
import { ref, computed } from "vue";
import * as api from "../api";
import type { Identity } from "../types";

export const useIdentitiesStore = defineStore("identities", () => {
  const identities = ref<Identity[]>([]);
  const loaded = ref(false);
  const loading = ref(false);
  const loadError = ref("");
  let pendingLoad: Promise<void> | null = null;
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
    pendingLoad = api.listIdentities().then(items => {
      if (!Array.isArray(items)) throw new Error("Invalid identity list response");
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
    identities.value.push(saved);
    return saved;
  }

  async function updateIdentity(identity: Identity) {
    const saved = await api.updateIdentity(identity);
    const idx = identities.value.findIndex((i) => i.id === saved.id);
    if (idx >= 0) identities.value[idx] = saved;
    return saved;
  }

  async function deleteIdentity(id: string) {
    await api.deleteIdentity(id);
    identities.value = identities.value.filter((i) => i.id !== id);
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
  };
});
