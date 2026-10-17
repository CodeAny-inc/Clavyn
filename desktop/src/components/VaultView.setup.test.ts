import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, describe, expect, it, vi } from "vitest";
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

function buttonWithText(wrapper: VueWrapper, text: string) {
  const button = wrapper.findAll("button").find((item) => item.text().trim() === text);
  expect(button, `Missing button: ${text}`).toBeDefined();
  return button!;
}

async function mountSetup() {
  const pinia = createPinia();
  setActivePinia(pinia);
  const vault = useVaultStore();
  vault.initialized = false;
  vault.unlocked = false;
  vault.biometricAvailable = false;
  vault.biometricEnabled = false;
  vi.spyOn(vault, "checkStatus").mockResolvedValue();

  const wrapper = mount(VaultView, { global: { plugins: [pinia] } });
  await flushPromises();
  return wrapper;
}

describe("VaultView setup passphrase length", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("refuses four emoji, which are eight UTF-16 units but four characters", async () => {
    const wrapper = await mountSetup();
    await wrapper.get("#setup-pass").setValue("\u{1F511}".repeat(4));
    await wrapper.get("#setup-confirm").setValue("\u{1F511}".repeat(4));

    buttonWithText(wrapper, "Create Vault").trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("Passphrase must be at least 8 characters");
    expect(api.initializeVault).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("accepts eight emoji", async () => {
    const wrapper = await mountSetup();
    const passphrase = "\u{1F511}".repeat(8);
    await wrapper.get("#setup-pass").setValue(passphrase);
    await wrapper.get("#setup-confirm").setValue(passphrase);

    buttonWithText(wrapper, "Create Vault").trigger("click");
    await flushPromises();

    expect(api.initializeVault).toHaveBeenCalledWith(passphrase);
    wrapper.unmount();
  });
});
