import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { describe, expect, it, vi } from "vitest";
import VaultView from "./VaultView.vue";
import { useVaultStore } from "../stores/vault";

describe("VaultView biometric settings", () => {
  it("shows a startup Keychain error when biometric state cannot be reconciled", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();

    vault.initialized = true;
    vault.unlocked = false;
    vault.biometricAvailable = true;
    vault.biometricEnabled = false;

    vi.spyOn(vault, "checkStatus").mockImplementation(async () => {
      vault.error = "Error: Keychain unavailable";
    });

    const wrapper = mount(VaultView, {
      global: {
        plugins: [pinia],
      },
    });
    await flushPromises();

    expect(wrapper.text()).toContain("Error: Keychain unavailable");

    wrapper.unmount();
  });

  it("does not auto-prompt when Touch ID is enrolled but temporarily unavailable", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();

    vault.initialized = true;
    vault.unlocked = false;
    vault.biometricAvailable = false;
    vault.biometricEnabled = true;

    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);
    const unlockWithBiometric = vi
      .spyOn(vault, "unlockWithBiometric")
      .mockResolvedValue(undefined);

    const wrapper = mount(VaultView, {
      global: {
        plugins: [pinia],
      },
    });
    await flushPromises();

    expect(unlockWithBiometric).not.toHaveBeenCalled();
    expect(wrapper.text()).not.toContain("Unlock with Touch ID");
    expect(wrapper.text()).toContain("Master passphrase");

    wrapper.unmount();
  });

  it("keeps enrolled biometric settings accessible while Touch ID is unavailable", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();

    vault.initialized = true;
    vault.unlocked = true;
    vault.biometricAvailable = false;
    vault.biometricEnabled = true;

    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);

    const wrapper = mount(VaultView, {
      global: {
        plugins: [pinia],
      },
    });
    await flushPromises();

    expect(wrapper.text()).toContain("Touch ID / Biometric Unlock");
    expect(wrapper.text()).toContain("Touch ID is currently unavailable");
    expect(
      wrapper
        .findAll("button")
        .some((button) => button.text().includes("Disable biometric unlock")),
    ).toBe(true);

    wrapper.unmount();
  });

  it("blocks passphrase unlock while Touch ID is pending", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();

    vault.initialized = true;
    vault.unlocked = false;
    vault.biometricAvailable = true;
    vault.biometricEnabled = true;

    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);
    let finishBiometric!: () => void;
    const unlockWithBiometric = vi
      .spyOn(vault, "unlockWithBiometric")
      .mockImplementation(
        () =>
          new Promise<void>((resolve) => {
            finishBiometric = resolve;
          }),
      );
    const unlock = vi.spyOn(vault, "unlock").mockResolvedValue(undefined);

    const wrapper = mount(VaultView, {
      global: {
        plugins: [pinia],
      },
    });
    await flushPromises();

    expect(unlockWithBiometric).toHaveBeenCalledTimes(1);
    expect((wrapper.get("#unlock-pass").element as HTMLInputElement).disabled).toBe(true);

    const passwordUnlockButton = wrapper
      .findAll("button")
      .find((button) => button.text().trim() === "Unlock");
    expect(passwordUnlockButton).toBeDefined();
    expect((passwordUnlockButton!.element as HTMLButtonElement).disabled).toBe(true);

    await passwordUnlockButton!.trigger("click");
    expect(unlock).not.toHaveBeenCalled();

    finishBiometric();
    await flushPromises();
    wrapper.unmount();
  });

  it("shows the Keychain error when disabling biometric unlock fails", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();

    vault.initialized = true;
    vault.unlocked = true;
    vault.biometricAvailable = true;
    vault.biometricEnabled = true;

    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);
    const disableBiometric = vi
      .spyOn(vault, "disableBiometric")
      .mockRejectedValue(new Error("Keychain delete failed"));

    const wrapper = mount(VaultView, {
      global: {
        plugins: [pinia],
      },
    });
    await flushPromises();

    const disableButton = wrapper
      .findAll("button")
      .find((button) => button.text().includes("Disable biometric unlock"));
    expect(disableButton).toBeDefined();

    await disableButton!.trigger("click");
    await flushPromises();

    expect(disableBiometric).toHaveBeenCalledTimes(1);
    expect(wrapper.text()).toContain("Error: Keychain delete failed");

    wrapper.unmount();
  });

  it("serializes Touch ID enrollment and prevents cancel while it is pending", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();

    vault.initialized = true;
    vault.unlocked = true;
    vault.biometricAvailable = true;
    vault.biometricEnabled = false;
    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);

    let finishEnable!: () => void;
    const enableBiometric = vi
      .spyOn(vault, "enableBiometric")
      .mockImplementation(
        () =>
          new Promise<void>((resolve) => {
            finishEnable = resolve;
          }),
      );

    const wrapper = mount(VaultView, {
      global: {
        plugins: [pinia],
      },
    });
    await flushPromises();

    const openButton = wrapper
      .findAll("button")
      .find((button) => button.text().includes("Enable biometric unlock"));
    expect(openButton).toBeDefined();
    await openButton!.trigger("click");

    const passphraseInput = wrapper.get("#bio-pass");
    await passphraseInput.setValue("correct horse battery staple");

    const enableButton = wrapper
      .findAll("button")
      .find((button) => button.text().trim() === "Enable");
    expect(enableButton).toBeDefined();
    await enableButton!.trigger("click");
    await flushPromises();

    expect(enableBiometric).toHaveBeenCalledWith("correct horse battery staple");
    expect((wrapper.get("#bio-pass").element as HTMLInputElement).disabled).toBe(true);

    const pendingEnableButton = wrapper
      .findAll("button")
      .find((button) => button.text().includes("Enabling..."));
    const cancelButton = wrapper
      .findAll("button")
      .find((button) => button.text().trim() === "Cancel");

    expect(pendingEnableButton).toBeDefined();
    expect((pendingEnableButton!.element as HTMLButtonElement).disabled).toBe(true);
    expect(cancelButton).toBeDefined();
    expect((cancelButton!.element as HTMLButtonElement).disabled).toBe(true);

    await cancelButton!.trigger("click");
    expect(wrapper.find("#bio-pass").exists()).toBe(true);

    finishEnable();
    await flushPromises();

    expect(wrapper.find("#bio-pass").exists()).toBe(false);
    wrapper.unmount();
  });

  it("clears the entered master passphrase when biometric enrollment is canceled", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();

    vault.initialized = true;
    vault.unlocked = true;
    vault.biometricAvailable = true;
    vault.biometricEnabled = false;
    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);

    const wrapper = mount(VaultView, {
      global: {
        plugins: [pinia],
      },
    });
    await flushPromises();

    const openEnrollment = async () => {
      const openButton = wrapper
        .findAll("button")
        .find((button) => button.text().includes("Enable biometric unlock"));
      expect(openButton).toBeDefined();
      await openButton!.trigger("click");
      await flushPromises();
    };

    await openEnrollment();
    await wrapper.get("#bio-pass").setValue("correct horse battery staple");

    const cancelButton = wrapper
      .findAll("button")
      .find((button) => button.text().trim() === "Cancel");
    expect(cancelButton).toBeDefined();
    await cancelButton!.trigger("click");
    await flushPromises();

    expect(wrapper.find("#bio-pass").exists()).toBe(false);

    await openEnrollment();
    expect((wrapper.get("#bio-pass").element as HTMLInputElement).value).toBe("");

    wrapper.unmount();
  });
});

describe("VaultView setup", () => {
  it("creates the vault once when Create Vault is clicked twice", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();
    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);
    let finish!: () => void;
    const initialize = vi
      .spyOn(vault, "initialize")
      .mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));

    const wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();
    await wrapper.get("#setup-pass").setValue("correct horse");
    await wrapper.get("#setup-confirm").setValue("correct horse");

    const create = () => wrapper.findAll("button").find((b) => b.text().includes("Create Vault"))!;
    await create().trigger("click");
    await create().trigger("click");
    await flushPromises();

    expect(initialize).toHaveBeenCalledTimes(1);
    expect(create().attributes("disabled")).toBeDefined();

    finish();
    await flushPromises();
    expect(create().attributes("disabled")).toBeUndefined();

    wrapper.unmount();
  });
});
