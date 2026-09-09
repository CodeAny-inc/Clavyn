import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
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

describe("VaultView sensitive form lifecycle", () => {
  let pinia: ReturnType<typeof createPinia>;
  let wrapper: VueWrapper | undefined;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    vi.resetAllMocks();
    vi.mocked(api.unlockVault).mockResolvedValue(undefined);
    vi.mocked(api.lockVault).mockResolvedValue(undefined);
    vi.mocked(api.unlockWithBiometric).mockResolvedValue(true);
    const vault = useVaultStore();
    vault.initialized = true;
    vault.biometricAvailable = true;
    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = undefined;
    vi.restoreAllMocks();
  });

  it("requires a fresh password after typed password -> Touch ID -> lock", async () => {
    const vault = useVaultStore();
    vault.biometricAvailable = false;
    vault.biometricEnabled = true;
    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    // Make Touch ID available after mount so this exercises the user's choice,
    // not the initial automatic biometric attempt.
    vault.biometricAvailable = true;
    await nextTick();
    await wrapper.get("#unlock-pass").setValue("correct horse battery staple");
    await buttonWithText(wrapper, "Unlock with Touch ID").trigger("click");
    await flushPromises();
    expect(vault.unlocked).toBe(true);
    expect(api.unlockWithBiometric).toHaveBeenCalledTimes(1);

    await buttonWithText(wrapper, "Lock").trigger("click");
    await flushPromises();
    expect(vault.unlocked).toBe(false);
    expect((wrapper.get("#unlock-pass").element as HTMLInputElement).value).toBe("");
    const passwordButton = buttonWithText(wrapper, "Unlock");
    expect((passwordButton.element as HTMLButtonElement).disabled).toBe(true);
    await passwordButton.trigger("click");
    await wrapper.get("#unlock-pass").trigger("keydown.enter");
    expect(api.unlockVault).not.toHaveBeenCalled();
    expect(vault.unlocked).toBe(false);
  });

  it.each(["manual", "automatic", "same-tick"] as const)(
    "clears an enrollment password on a %s lock transition",
    async (lockSource) => {
      const vault = useVaultStore();
      vault.unlocked = true;
      wrapper = mount(VaultView, { global: { plugins: [pinia] } });
      await flushPromises();
      await buttonWithText(wrapper, "Enable biometric unlock").trigger("click");
      await wrapper.get("#bio-pass").setValue("correct horse battery staple");

      if (lockSource === "manual") {
        await buttonWithText(wrapper, "Lock").trigger("click");
        await flushPromises();
      } else if (lockSource === "automatic") {
        // Auto-lock calls the store rather than the component's Lock handler.
        await vault.lock();
      } else {
        // A batched watcher can miss this false -> true round trip entirely.
        vault.unlocked = false;
        vault.unlocked = true;
      }

      // The settings form is hidden when locked, but its component stays mounted.
      // Inspect the retained setup state before any later unlock can clear it.
      const state = wrapper.vm as unknown as {
        biometricPassphrase: string;
        enableBiometricMode: boolean;
      };
      expect(state.biometricPassphrase).toBe("");
      expect(state.enableBiometricMode).toBe(false);

      if (!vault.unlocked) await vault.unlock("freshly entered passphrase");
      await nextTick();
      expect(wrapper.find("#bio-pass").exists()).toBe(false);
      await buttonWithText(wrapper, "Enable biometric unlock").trigger("click");
      expect((wrapper.get("#bio-pass").element as HTMLInputElement).value).toBe("");
    },
  );
});
