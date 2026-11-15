import { defineStore } from "pinia";
import { ref, computed } from "vue";
import * as api from "../api";
import type { Host, HostGroup } from "../types";
import { useIdentitiesStore } from "./identities";

export const useHostsStore = defineStore("hosts", () => {
  const hosts = ref<Host[]>([]);
  const groups = ref<HostGroup[]>([]);
  const searchQuery = ref("");
  const selectedGroupId = ref<string | null>(null);
  // Lets a view tell "nothing stored" apart from "not read yet" and "could
  // not be read", which all leave `hosts` empty.
  const loaded = ref(false);
  const loading = ref(false);
  const loadError = ref("");
  let pendingLoad: Promise<void> | null = null;
  // Bumped whenever a host or group has been added, changed or deleted locally.
  // A read that started before the bump carries a snapshot taken before that
  // change, so it is discarded rather than written over newer state.
  //
  // Counted per list, because the two reads are independent: adding a host
  // says nothing about the group list that came back with it, and one counter
  // would throw away a current answer for the collection nothing touched.
  let hostsRevision = 0;
  let groupsRevision = 0;
  // Deletes that cascade to other records do not bump the revisions. The
  // backend applies the cascade in the same write as the delete, so a read that
  // was already in flight when the delete completed can still carry the links
  // it removed. While any read is in flight these remember what such deletes
  // removed, and each snapshot is reconciled with them as it arrives. Dropping
  // the snapshot instead would leave a list that had not loaded yet empty.
  //
  // They are cleared when the last read settles, not when the aggregate load
  // does: Promise.all rejects as soon as one read fails, while the other can
  // still publish its snapshot afterwards, and a delete can land in that gap.
  const unlinkedIdentityIds = new Set<string>();
  const deletedGroupIds = new Set<string>();
  let readsInFlight = 0;

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
  //
  // A result is not dropped for a delete that cascades (an identity or a
  // group): it is reconciled with what that delete removed, so a delete during
  // the first load still leaves the lists loaded.
  function load(): Promise<void> {
    if (pendingLoad) return pendingLoad;
    const hostsStartedAt = hostsRevision;
    const groupsStartedAt = groupsRevision;
    loading.value = true;
    loadError.value = "";
    pendingLoad = Promise.all([
      tracked(api.listHosts(), "hosts", (loaded) => {
        if (hostsRevision === hostsStartedAt) hosts.value = withoutCascaded(loaded);
      }),
      tracked(api.listGroups(), "groups", (loaded) => {
        if (groupsRevision === groupsStartedAt) {
          groups.value = loaded.filter((g) => !deletedGroupIds.has(g.id));
        }
      }),
    ])
      .then(() => {
        loaded.value = true;
      })
      .catch((e) => {
        loadError.value = String(e);
        throw e;
      })
      .finally(() => {
        loading.value = false;
        pendingLoad = null;
      });
    return pendingLoad;
  }

  // Applies one read's snapshot, tagging a failure with which list it was. The
  // read counts as in flight until the snapshot has been applied or has failed,
  // and the cascade records are dropped once none is left.
  function tracked<T>(read: Promise<T>, label: string, apply: (loaded: T) => void): Promise<void> {
    readsInFlight += 1;
    return read
      .then(apply, (e) => {
        throw `${label}: ${e}`;
      })
      .finally(() => {
        readsInFlight -= 1;
        if (readsInFlight === 0) {
          unlinkedIdentityIds.clear();
          deletedGroupIds.clear();
        }
      });
  }

  // Applies, to a host snapshot that was read before them, the links that the
  // deletes completed since then removed.
  function withoutCascaded(snapshot: Host[]): Host[] {
    for (const host of snapshot) {
      if (host.identity_id && unlinkedIdentityIds.has(host.identity_id)) host.identity_id = null;
      if (host.group_id && deletedGroupIds.has(host.group_id)) host.group_id = null;
    }
    return snapshot;
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
    // unassigned. A load in flight is reconciled with this when it lands.
    if (readsInFlight > 0) deletedGroupIds.add(id);
    groups.value = groups.value.filter((g) => g.id !== id);
    hosts.value.forEach((h) => {
      if (h.group_id === id) h.group_id = null;
    });
    // The backend ungroups identities in the same write. Mirror it, or the next
    // edit of such an identity sends the deleted group id straight back.
    useIdentitiesStore().ungroup(id);
  }

  // The backend unlinks hosts from an identity it deletes; this applies the
  // same change locally so those hosts do not keep pointing at nothing. A load
  // in flight is reconciled with this when it lands.
  function unlinkIdentity(identityId: string) {
    if (readsInFlight > 0) unlinkedIdentityIds.add(identityId);
    hosts.value.forEach((h) => {
      if (h.identity_id === identityId) h.identity_id = null;
    });
  }

  return {
    hosts,
    groups,
    searchQuery,
    selectedGroupId,
    filteredHosts,
    loaded,
    loading,
    loadError,
    load,
    addHost,
    updateHost,
    deleteHost,
    addGroup,
    deleteGroup,
    unlinkIdentity,
  };
});
