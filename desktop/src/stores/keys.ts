import { defineStore } from "pinia";
import { ref } from "vue";
import * as api from "../api";
import type { KeyMeta } from "../types";

export const useKeysStore = defineStore("keys", () => {
  const keys = ref<KeyMeta[]>([]);

  async function load() {
    keys.value = await api.listKeys();
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
    keys.value = [];
  }

  return { keys, load, generateKey, importKey, deleteKey, clear };
});
