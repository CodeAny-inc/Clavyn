import { defineStore } from "pinia";
import { ref } from "vue";
import * as api from "../api";
import type { KeyMeta } from "../types";

export const useKeysStore = defineStore("keys", () => {
  const keys = ref<KeyMeta[]>([]);
  // Bumped by `clear`, so a load that was in flight when the vault locked
  // cannot put the keys back afterwards.
  let generation = 0;

  async function load() {
    const started = generation;
    const loaded = await api.listKeys();
    if (started === generation) keys.value = loaded;
  }

  // An add that was in flight when the vault locked must not put a key back
  // into the list the lock cleared: the pickers in HostForm and
  // IdentityManager read that list, and would offer the key while locked.
  async function generateKey(label: string) {
    const started = generation;
    const key = await api.generateKey(label);
    if (started === generation) keys.value.push(key);
    return key;
  }

  async function importKey(
    label: string,
    opensshPrivate: string,
    keyPassphrase: string | null,
  ) {
    const started = generation;
    const key = await api.importKey(label, opensshPrivate, keyPassphrase);
    if (started === generation) keys.value.push(key);
    return key;
  }

  async function deleteKey(keyId: string) {
    await api.deleteKey(keyId);
    keys.value = keys.value.filter((k) => k.id !== keyId);
  }

  function clear() {
    generation += 1;
    keys.value = [];
  }

  return { keys, load, generateKey, importKey, deleteKey, clear };
});
