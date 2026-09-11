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
  // Bumped whenever a mutation has been applied locally. A read that started
  // before the bump carries a snapshot taken before that change, so it is
  // discarded rather than written over newer state.
  //
  // Counted per list, because the two reads are independent: adding a host
  // says nothing about the group list that came back with it, and one counter
  // would throw away a current answer for the collection nothing touched.
  let hostsRevision = 0;
  let groupsRevision = 0;

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
  //
  // A load replaces each list wholesale, so it must not land on top of a change
  // that completed while it was in flight: adding a host during startup would
  // otherwise see the new row appear and then vanish when the older snapshot
  // arrived. Sharing one in-flight read across callers widens that window, so a
  // result is dropped if its own collection was mutated after the read began.
  // Each result is checked on its own, so a host being added does not also
  // throw away the group list that arrived alongside it.
  function load(): Promise<void> {
    if (pendingLoad) return pendingLoad;
    const hostsStartedAt = hostsRevision;
    const groupsStartedAt = groupsRevision;
    pendingLoad = Promise.all([
      api.listHosts().then((loaded) => {
        if (hostsRevision === hostsStartedAt) hosts.value = loaded;
      }),
      api.listGroups().then((loaded) => {
        if (groupsRevision === groupsStartedAt) groups.value = loaded;
      }),
    ])
      .then(() => undefined)
      .finally(() => {
        pendingLoad = null;
      });
    return pendingLoad;
  }

  async function addHost(host: Host) {
    const saved = await api.addHost(host);
    hostsRevision += 1;
    hosts.value.push(saved);
    return saved;
  }

  async function updateHost(host: Host) {
    const saved = await api.updateHost(host);
    hostsRevision += 1;
    const idx = hosts.value.findIndex((h) => h.id === saved.id);
    if (idx >= 0) hosts.value[idx] = saved;
    return saved;
  }

  async function deleteHost(id: string) {
    await api.deleteHost(id);
    hostsRevision += 1;
    hosts.value = hosts.value.filter((h) => h.id !== id);
  }

  async function addGroup(name: string) {
    const group = await api.addGroup(name);
    groupsRevision += 1;
    groups.value.push(group);
    return group;
  }

  async function deleteGroup(id: string) {
    await api.deleteGroup(id);
    // Touches both lists: the group goes, and every host that pointed at it is
    // unassigned.
    groupsRevision += 1;
    hostsRevision += 1;
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
