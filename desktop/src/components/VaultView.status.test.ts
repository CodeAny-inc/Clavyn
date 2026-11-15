import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { describe, expect, it, vi } from "vitest";
import * as api from "../api";
import { useVaultStore } from "../stores/vault";
import VaultView from "./VaultView.vue";

vi.mock("../api", () => ({
  vaultIsInitialized: vi.fn(),
  isVaultUnlocked: vi.fn(),
  initializeVault: vi.fn(),
  unlockVault: vi.fn(),
  lockVault: vi.fn(),
  resetVault: vi.fn(),
  biometricAvailable: vi.fn(),
  biometricPassphraseStored: vi.fn(),
  unlockWithBiometric: vi.fn(),
  storeBiometricPassphrase: vi.fn(),
  clearBiometricPassphrase: vi.fn(),
}));

describe("VaultView with a failed status probe", () => {
  it("shows Retry instead of the unlock form and does not try biometric unlock", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();
    vault.initialized = true;
    vault.unlocked = false;
    vault.statusUnavailable = true;
    vault.biometricAvailable = true;
    vault.biometricEnabled = true;
    vi.spyOn(vault, "checkStatus").mockResolvedValue();

    const wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    expect(wrapper.text()).toContain("Vault status unknown");
    expect(wrapper.text()).toContain("Retry");
    expect(wrapper.text()).not.toContain("Vault locked");
    expect(api.unlockWithBiometric).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
