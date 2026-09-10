import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useKeysStore } from "./keys";
import { useVaultStore } from "./vault";

vi.mock("../api", () => ({
  resetVault: vi.fn(),
  biometricPassphraseStored: vi.fn(),
}));

import * as api from "../api";

function seedDestroyedKeyMetadata() {
  const keys = useKeysStore();
  keys.keys = [
    {
      id: "old-key",
      label: "Old vault key",
      key_type: "ed25519",
      fingerprint: "SHA256:old",
      public_key_base64: "ssh-ed25519 AAAAold",
    },
  ];
  return keys;
}

describe("vault reset key-cache invalidation", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it("clears cached key metadata after a successful destructive reset", async () => {
    const keys = seedDestroyedKeyMetadata();
    const vault = useVaultStore();
    vault.initialized = true;
    vault.unlocked = true;
    vi.mocked(api.resetVault).mockResolvedValue(undefined);

    await vault.reset("my-passphrase");

    expect(keys.keys).toEqual([]);
    expect(vault.initialized).toBe(false);
    expect(vault.unlocked).toBe(false);
  });

  it("also clears cached keys when deletion succeeded but directory fsync failed", async () => {
    const keys = seedDestroyedKeyMetadata();
    const vault = useVaultStore();
    vault.initialized = true;
    vault.unlocked = true;
    vi.mocked(api.resetVault).mockRejectedValue(
      new Error(
        "[vault-reset-durability] vault was deleted but directory sync failed",
      ),
    );

    await expect(vault.reset("my-passphrase")).rejects.toThrow(
      "vault was deleted but directory sync failed",
    );

    expect(keys.keys).toEqual([]);
    expect(vault.initialized).toBe(false);
    expect(vault.unlocked).toBe(false);
  });
});
