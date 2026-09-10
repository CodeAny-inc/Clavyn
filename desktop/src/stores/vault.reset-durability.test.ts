import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useVaultStore } from "./vault";

vi.mock("../api", () => ({
  resetVault: vi.fn(),
  biometricPassphraseStored: vi.fn(),
}));

import * as api from "../api";

describe("vault reset durability failure", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it("fails closed when reset reports that deletion already crossed the destructive boundary", async () => {
    const store = useVaultStore();
    store.initialized = true;
    store.unlocked = true;
    store.biometricEnabled = true;
    vi.mocked(api.resetVault).mockRejectedValue(
      new Error(
        "[vault-reset-durability] vault was deleted but directory sync failed",
      ),
    );

    await expect(store.reset("my-passphrase")).rejects.toThrow(
      "vault was deleted but directory sync failed",
    );

    expect(store.initialized).toBe(false);
    expect(store.unlocked).toBe(false);
    expect(store.biometricEnabled).toBe(false);
    expect(store.needsSetup).toBe(true);
    expect(store.error).toBeNull();
    expect(api.biometricPassphraseStored).not.toHaveBeenCalled();
  });
});
