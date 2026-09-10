import { defineStore } from "pinia";
import { ref, computed } from "vue";
import * as api from "../api";
import type { Host, HostGroup } from "../types";

export const useHostsStore = defineStore("hosts", () => {
  const hosts = ref<Host[]>([]);
  const groups = ref<HostGroup[]>([]);
  const searchQuery = ref("");
  const selectedGroupId = ref<string | null>(null);
  let pendingLoad: Promise<void> | null = null;

  const filteredHosts = computed(() => {
    let result = hosts.value;
    if (selectedGroupId.value) {
      result = result.filter((h) => h.group_id === selectedGroupId.value);
    }
    if (searchQuery.value.trim()) {
      const q = searchQuery.value.toLowerCase();
      result = result.filter(
        (h) =>
          h.label.toLowerCase().includes(q) ||
          h.hostname.toLowerCase().includes(q) ||
          h.username.toLowerCase().includes(q) ||
          h.tags.some((t) => t.toLowerCase().includes(q)),
      );
    }
    return result;
  });

  // App startup and every host-facing view call load(). Concurrent callers share
  // one in-flight pair of reads rather than issuing duplicate IPC, and the two
  // reads are independent so they go out together.
  function load(): Promise<void> {
    if (pendingLoad) return pendingLoad;
    pendingLoad = Promise.all([api.listHosts(), api.listGroups()])
      .then(([loadedHosts, loadedGroups]) => {
        hosts.value = loadedHosts;
        groups.value = loadedGroups;
      })
      .finally(() => {
        pendingLoad = null;
      });
    return pendingLoad;
  }

  async function addHost(host: Host) {
    const saved = await api.addHost(host);
    hosts.value.push(saved);
    return saved;
  }

  async function updateHost(host: Host) {
    const saved = await api.updateHost(host);
    const idx = hosts.value.findIndex((h) => h.id === saved.id);
    if (idx >= 0) hosts.value[idx] = saved;
    return saved;
  }

  async function deleteHost(id: string) {
    await api.deleteHost(id);
    hosts.value = hosts.value.filter((h) => h.id !== id);
  }

  async function addGroup(name: string) {
    const group = await api.addGroup(name);
    groups.value.push(group);
    return group;
  }

  async function deleteGroup(id: string) {
    await api.deleteGroup(id);
    groups.value = groups.value.filter((g) => g.id !== id);
    hosts.value.forEach((h) => {
      if (h.group_id === id) h.group_id = null;
    });
  }

  return {
    hosts,
    groups,
    searchQuery,
    selectedGroupId,
    filteredHosts,
    load,
    addHost,
    updateHost,
    deleteHost,
    addGroup,
    deleteGroup,
  };
});
