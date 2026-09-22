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

  async function generateKey(label: string) {
    const key = await api.generateKey(label);
    keys.value.push(key);
    return key;
  }

  async function importKey(
    label: string,
    opensshPrivate: string,
    keyPassphrase: string | null,
  ) {
    const key = await api.importKey(label, opensshPrivate, keyPassphrase);
    keys.value.push(key);
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
