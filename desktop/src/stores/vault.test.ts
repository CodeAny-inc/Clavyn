import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { useVaultStore } from "./vault";

vi.mock("../api", () => ({
  vaultIsInitialized: vi.fn(() => Promise.resolve(false)),
  isVaultUnlocked: vi.fn(() => Promise.resolve(false)),
  initializeVault: vi.fn(() => Promise.resolve(true)),
  unlockVault: vi.fn(() => Promise.resolve()),
  lockVault: vi.fn(() => Promise.resolve()),
  resetVault: vi.fn(() => Promise.resolve()),
  biometricAvailable: vi.fn(() => Promise.resolve(false)),
  biometricPassphraseStored: vi.fn(() => Promise.resolve(false)),
  storeBiometricPassphrase: vi.fn(() => Promise.resolve()),
  unlockWithBiometric: vi.fn(() => Promise.resolve(true)),
  clearBiometricPassphrase: vi.fn(() => Promise.resolve()),
}));

import * as api from "../api";

describe("vault store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    vi.mocked(api.vaultIsInitialized).mockResolvedValue(false);
    vi.mocked(api.isVaultUnlocked).mockResolvedValue(false);
    vi.mocked(api.initializeVault).mockResolvedValue(true);
    vi.mocked(api.unlockVault).mockResolvedValue(undefined);
    vi.mocked(api.lockVault).mockResolvedValue(undefined);
    vi.mocked(api.resetVault).mockResolvedValue(undefined);
    vi.mocked(api.biometricAvailable).mockResolvedValue(false);
    vi.mocked(api.biometricPassphraseStored).mockResolvedValue(false);
    vi.mocked(api.unlockWithBiometric).mockResolvedValue(true);
    vi.mocked(api.storeBiometricPassphrase).mockResolvedValue(undefined);
    vi.mocked(api.clearBiometricPassphrase).mockResolvedValue(undefined);
  });

  describe("initial state", () => {
    it("starts uninitialized and locked", () => {
      const store = useVaultStore();
      expect(store.initialized).toBe(false);
      expect(store.unlocked).toBe(false);
      expect(store.error).toBeNull();
      expect(store.needsSetup).toBe(true);
      expect(store.needsUnlock).toBe(false);
    });
  });

  describe("checkStatus", () => {
    it("checks vault initialization and unlock status", async () => {
      const store = useVaultStore();
      vi.mocked(api.vaultIsInitialized).mockResolvedValue(true);
      vi.mocked(api.isVaultUnlocked).mockResolvedValue(false);

      await store.checkStatus();

      expect(store.initialized).toBe(true);
      expect(store.unlocked).toBe(false);
      expect(store.needsSetup).toBe(false);
      expect(store.needsUnlock).toBe(true);
    });

    it("reads both status probes together and publishes them as one snapshot", async () => {
      const store = useVaultStore();
      const order: string[] = [];
      let releaseInitialized!: (value: boolean) => void;
      vi.mocked(api.vaultIsInitialized).mockImplementationOnce(() => {
        order.push("initialized");
        return new Promise<boolean>(resolve => {
          releaseInitialized = resolve;
        });
      });
      vi.mocked(api.isVaultUnlocked).mockImplementationOnce(() => {
        order.push("unlocked");
        return Promise.resolve(true);
      });

      const status = store.checkStatus();
      expect(order).toEqual(["initialized", "unlocked"]);
      // Neither ref moves until both probes have answered.
      expect(store.initialized).toBe(false);
      expect(store.unlocked).toBe(false);
      expect(api.biometricAvailable).not.toHaveBeenCalled();

      releaseInitialized(true);
      await status;

      expect(store.initialized).toBe(true);
      expect(store.unlocked).toBe(true);
      expect(api.biometricAvailable).toHaveBeenCalledTimes(1);
    });

    it("does not attach a stale biometric credential to an uninitialized vault", async () => {
      const store = useVaultStore();
      vi.mocked(api.biometricAvailable).mockResolvedValue(true);
      vi.mocked(api.biometricPassphraseStored).mockResolvedValue(true);

      await store.checkStatus();

      expect(api.biometricPassphraseStored).not.toHaveBeenCalled();
      expect(store.initialized).toBe(false);
      expect(store.biometricAvailable).toBe(true);
      expect(store.biometricEnabled).toBe(false);
    });

    it("uses the protected Keychain item as biometric source of truth", async () => {
      const store = useVaultStore();
      vi.mocked(api.vaultIsInitialized).mockResolvedValue(true);
      vi.mocked(api.biometricAvailable).mockResolvedValue(true);
      vi.mocked(api.biometricPassphraseStored).mockResolvedValue(true);

      await store.checkStatus();

      expect(api.biometricPassphraseStored).toHaveBeenCalledTimes(1);
      expect(store.biometricAvailable).toBe(true);
      expect(store.biometricEnabled).toBe(true);
      expect(store.error).toBeNull();
    });

    it("keeps credential enrollment when Touch ID is temporarily unavailable", async () => {
      const store = useVaultStore();
      vi.mocked(api.vaultIsInitialized).mockResolvedValue(true);
      vi.mocked(api.biometricAvailable).mockResolvedValue(false);
      vi.mocked(api.biometricPassphraseStored).mockResolvedValue(true);

      await store.checkStatus();

      expect(api.biometricPassphraseStored).toHaveBeenCalledTimes(1);
      expect(store.biometricAvailable).toBe(false);
      expect(store.biometricEnabled).toBe(true);
      expect(store.error).toBeNull();
    });

    it("recovers current Touch ID availability without losing enrollment state", async () => {
      const store = useVaultStore();
      store.initialized = true;
      vi.mocked(api.biometricPassphraseStored).mockResolvedValue(true);
      vi.mocked(api.biometricAvailable).mockResolvedValue(false);

      await store.refreshBiometricState();
      expect(store.biometricAvailable).toBe(false);
      expect(store.biometricEnabled).toBe(true);

      vi.mocked(api.biometricAvailable).mockResolvedValue(true);
      await store.refreshBiometricState();

      expect(api.biometricAvailable).toHaveBeenCalledTimes(2);
      expect(api.biometricPassphraseStored).toHaveBeenCalledTimes(2);
      expect(store.biometricAvailable).toBe(true);
      expect(store.biometricEnabled).toBe(true);
    });

    it("fails closed when the protected Keychain state cannot be queried", async () => {
      const store = useVaultStore();
      vi.mocked(api.vaultIsInitialized).mockResolvedValue(true);
      vi.mocked(api.biometricAvailable).mockResolvedValue(true);
      vi.mocked(api.biometricPassphraseStored).mockRejectedValue(
        new Error("Keychain unavailable"),
      );

      await store.checkStatus();

      expect(store.biometricAvailable).toBe(false);
      expect(store.biometricEnabled).toBe(false);
      expect(store.error).toBe("Error: Keychain unavailable");
    });
  });

  describe("initialize", () => {
    it("delegates stale-credential cleanup to the guarded backend initializer", async () => {
      const store = useVaultStore();
      store.biometricEnabled = true;

      await store.initialize("my-passphrase");

      expect(api.clearBiometricPassphrase).not.toHaveBeenCalled();
      expect(api.initializeVault).toHaveBeenCalledWith("my-passphrase");
      expect(store.biometricEnabled).toBe(false);
      expect(store.initialized).toBe(true);
      expect(store.unlocked).toBe(true);
      expect(store.error).toBeNull();
    });

    it("does not delete biometric credentials when backend initialization is rejected", async () => {
      const store = useVaultStore();
      store.biometricEnabled = true;
      vi.mocked(api.initializeVault).mockRejectedValue(
        new Error("vault already initialized"),
      );

      await expect(store.initialize("my-passphrase")).rejects.toThrow(
        "vault already initialized",
      );

      expect(api.clearBiometricPassphrase).not.toHaveBeenCalled();
      expect(store.biometricEnabled).toBe(true);
      expect(store.initialized).toBe(false);
      expect(store.unlocked).toBe(false);
      expect(store.error).toBe("Error: vault already initialized");
    });

    it("keeps a successfully created vault locked when a newer lock supersedes auto-unlock", async () => {
      const store = useVaultStore();
      vi.mocked(api.initializeVault).mockResolvedValue(false);

      await store.initialize("my-passphrase");

      expect(store.initialized).toBe(true);
      expect(store.unlocked).toBe(false);
      expect(store.needsUnlock).toBe(true);
      expect(store.error).toBeNull();
    });

    it("sets error on vault initialization failure", async () => {
      const store = useVaultStore();
      vi.mocked(api.initializeVault).mockRejectedValue(new Error("Weak passphrase"));

      await expect(store.initialize("weak")).rejects.toThrow("Weak passphrase");

      expect(store.initialized).toBe(false);
      expect(store.error).toBe("Error: Weak passphrase");
    });
  });

  describe("unlock", () => {
    it("unlocks the vault with a passphrase", async () => {
      const store = useVaultStore();
      store.initialized = true;

      await store.unlock("correct-passphrase");

      expect(api.unlockVault).toHaveBeenCalledWith("correct-passphrase");
      expect(store.unlocked).toBe(true);
      expect(store.error).toBeNull();
    });

    it("sets error and rethrows on wrong passphrase", async () => {
      const store = useVaultStore();
      store.initialized = true;
      vi.mocked(api.unlockVault).mockRejectedValue(new Error("Wrong passphrase"));

      await expect(store.unlock("wrong")).rejects.toThrow("Wrong passphrase");
      expect(store.unlocked).toBe(false);
      expect(store.error).toBe("Error: Wrong passphrase");
    });
  });

  describe("unlockWithBiometric", () => {
    it("disables biometric UI when the protected credential is gone after a failed unlock", async () => {
      const store = useVaultStore();
      store.initialized = true;
      store.biometricAvailable = true;
      store.biometricEnabled = true;
      vi.mocked(api.unlockWithBiometric).mockRejectedValue(
        new Error("Touch ID credential invalidated"),
      );
      vi.mocked(api.biometricPassphraseStored).mockResolvedValue(false);

      await expect(store.unlockWithBiometric()).rejects.toThrow(
        "Touch ID credential invalidated",
      );

      expect(api.biometricPassphraseStored).toHaveBeenCalledTimes(1);
      expect(store.biometricEnabled).toBe(false);
      expect(store.error).toBe("Error: Touch ID credential invalidated");
    });

    it("keeps biometric enrollment when a failed attempt leaves the credential stored", async () => {
      const store = useVaultStore();
      store.initialized = true;
      store.biometricAvailable = true;
      store.biometricEnabled = true;
      vi.mocked(api.unlockWithBiometric).mockRejectedValue(
        new Error("Touch ID was canceled by the user"),
      );
      vi.mocked(api.biometricPassphraseStored).mockResolvedValue(true);

      await expect(store.unlockWithBiometric()).rejects.toThrow(
        "Touch ID was canceled by the user",
      );

      expect(store.biometricEnabled).toBe(true);
      expect(store.error).toBe("Error: Touch ID was canceled by the user");
    });

    it("fails closed if the post-failure Keychain probe also fails", async () => {
      const store = useVaultStore();
      store.initialized = true;
      store.biometricAvailable = true;
      store.biometricEnabled = true;
      vi.mocked(api.unlockWithBiometric).mockRejectedValue(
        new Error("Touch ID authentication failed"),
      );
      vi.mocked(api.biometricPassphraseStored).mockRejectedValue(
        new Error("Keychain unavailable"),
      );

      await expect(store.unlockWithBiometric()).rejects.toThrow(
        "Touch ID authentication failed",
      );

      expect(store.biometricAvailable).toBe(false);
      expect(store.biometricEnabled).toBe(false);
      expect(store.error).toBe("Error: Touch ID authentication failed");
    });
  });

  describe("lock", () => {
    it("locks the vault", async () => {
      const store = useVaultStore();
      store.unlocked = true;

      await store.lock();

      expect(api.lockVault).toHaveBeenCalled();
      expect(store.unlocked).toBe(false);
    });
  });

  describe("reset", () => {
    it("destroys the vault and returns to the setup state", async () => {
      const store = useVaultStore();
      store.initialized = true;
      store.unlocked = true;
      store.biometricEnabled = true;

      await store.reset("my-passphrase");

      expect(api.resetVault).toHaveBeenCalledWith("my-passphrase");
      expect(store.initialized).toBe(false);
      expect(store.unlocked).toBe(false);
      expect(store.biometricEnabled).toBe(false);
      expect(store.needsSetup).toBe(true);
      expect(store.error).toBeNull();
    });

    it("keeps wrong-passphrase errors local and preserves enrolled biometric state", async () => {
      const store = useVaultStore();
      store.initialized = true;
      store.unlocked = true;
      store.biometricEnabled = true;
      vi.mocked(api.resetVault).mockRejectedValue(new Error("wrong passphrase"));
      vi.mocked(api.biometricPassphraseStored).mockResolvedValue(true);

      await expect(store.reset("wrong")).rejects.toThrow("wrong passphrase");

      expect(api.resetVault).toHaveBeenCalledWith("wrong");
      expect(api.biometricPassphraseStored).toHaveBeenCalledTimes(1);
      expect(store.initialized).toBe(true);
      expect(store.unlocked).toBe(true);
      expect(store.biometricEnabled).toBe(true);
      expect(store.error).toBeNull();
    });

    it("reconciles biometric state when reset fails after credential cleanup", async () => {
      const store = useVaultStore();
      store.initialized = true;
      store.unlocked = true;
      store.biometricEnabled = true;
      vi.mocked(api.resetVault).mockRejectedValue(
        new Error("failed to remove vault file"),
      );
      vi.mocked(api.biometricPassphraseStored).mockResolvedValue(false);

      await expect(store.reset("my-passphrase")).rejects.toThrow(
        "failed to remove vault file",
      );

      expect(api.biometricPassphraseStored).toHaveBeenCalledTimes(1);
      expect(store.initialized).toBe(true);
      expect(store.unlocked).toBe(true);
      expect(store.biometricEnabled).toBe(false);
      expect(store.error).toBeNull();
    });
  });

  describe("computed flags", () => {
    it("needsSetup is true when not initialized", () => {
      const store = useVaultStore();
      store.initialized = false;
      expect(store.needsSetup).toBe(true);
    });

    it("needsUnlock is true when initialized but not unlocked", () => {
      const store = useVaultStore();
      store.initialized = true;
      store.unlocked = false;
      expect(store.needsSetup).toBe(false);
      expect(store.needsUnlock).toBe(true);
    });

    it("needsSetup and needsUnlock are false when initialized and unlocked", () => {
      const store = useVaultStore();
      store.initialized = true;
      store.unlocked = true;
      expect(store.needsSetup).toBe(false);
      expect(store.needsUnlock).toBe(false);
    });
  });
});
