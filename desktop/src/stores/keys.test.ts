import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { useKeysStore } from "./keys";
import type { KeyMeta } from "../types";

const mockKey: KeyMeta = {
  id: "key-1",
  label: "My Laptop Key",
  key_type: "ed25519",
  fingerprint: "SHA256:abc123",
  public_key_base64: "ssh-ed25519 AAAA...",
};

const mockKey2: KeyMeta = {
  id: "key-2",
  label: "Server Key",
  key_type: "rsa",
  fingerprint: "SHA256:def456",
  public_key_base64: "ssh-rsa AAAA...",
};

vi.mock("../api", () => ({
  listKeys: vi.fn(() => Promise.resolve([mockKey, mockKey2])),
  generateKey: vi.fn((label: string) =>
    Promise.resolve({
      id: "new-key",
      label,
      key_type: "ed25519" as const,
      fingerprint: "SHA256:new",
      public_key_base64: "ssh-ed25519 BBBB...",
    }),
  ),
  importKey: vi.fn((label: string, _priv: string, _pass: string | null) =>
    Promise.resolve({
      id: "imported-key",
      label,
      key_type: "ed25519" as const,
      fingerprint: "SHA256:imported",
      public_key_base64: "ssh-ed25519 CCCC...",
    }),
  ),
  deleteKey: vi.fn(() => Promise.resolve()),
}));

describe("keys store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  describe("load", () => {
    it("loads keys from the API", async () => {
      const store = useKeysStore();
      await store.load();

      expect(store.keys).toHaveLength(2);
      expect(store.keys[0].label).toBe("My Laptop Key");
      expect(store.keys[1].key_type).toBe("rsa");
    });
  });

  describe("generateKey", () => {
    it("generates a new key and adds it to the store", async () => {
      const store = useKeysStore();
      const key = await store.generateKey("Work Key");

      expect(key.id).toBe("new-key");
      expect(key.label).toBe("Work Key");
      expect(key.key_type).toBe("ed25519");
      expect(store.keys).toContainEqual(key);
    });
  });

  describe("importKey", () => {
    it("imports a key and adds it to the store", async () => {
      const store = useKeysStore();
      const key = await store.importKey(
        "Imported Key",
        "-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n-----END OPENSSH PRIVATE KEY-----",
        null,
      );

      expect(key.id).toBe("imported-key");
      expect(key.label).toBe("Imported Key");
      expect(store.keys).toContainEqual(key);
    });

    it("imports a key with a passphrase", async () => {
      const store = useKeysStore();
      const key = await store.importKey("Encrypted Key", "fake-key-data", "mypass");

      expect(key.id).toBe("imported-key");
      expect(key.label).toBe("Encrypted Key");
    });
  });

  describe("deleteKey", () => {
    it("removes a key from the store", async () => {
      const store = useKeysStore();
      await store.load();
      expect(store.keys).toHaveLength(2);

      await store.deleteKey("key-1");
      expect(store.keys).toHaveLength(1);
      expect(store.keys.find((k) => k.id === "key-1")).toBeUndefined();
    });
  });

  describe("clear", () => {
    it("drops cached metadata for keys destroyed with the vault", async () => {
      const store = useKeysStore();
      await store.load();
      expect(store.keys).toHaveLength(2);

      store.clear();

      expect(store.keys).toEqual([]);
    });
  });
});
