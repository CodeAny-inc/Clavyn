import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";
import VaultView from "./VaultView.vue";
import { useVaultStore } from "../stores/vault";

function buttonWithText(wrapper: ReturnType<typeof mount>, text: string) {
  const button = wrapper.findAll("button").find((item) => item.text().trim() === text);
  expect(button, `Missing button: ${text}`).toBeDefined();
  return button!;
}

describe("VaultView vault reset", () => {
  let pinia: ReturnType<typeof createPinia>;
  let wrapper: ReturnType<typeof mount> | undefined;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();
    vault.initialized = true;
    vault.unlocked = true;
    vault.biometricAvailable = false;
    vault.biometricEnabled = false;
    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = undefined;
    vi.restoreAllMocks();
  });

  it("shows the Danger Zone with a Reset Vault button when unlocked", async () => {
    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    expect(wrapper.text()).toContain("Danger Zone");
    expect(wrapper.text()).toContain("Reset Vault");
  });

  it("reveals the passphrase confirmation form when Reset Vault is clicked", async () => {
    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    expect(wrapper.find("#reset-pass").exists()).toBe(false);

    await buttonWithText(wrapper, "Reset Vault").trigger("click");
    await flushPromises();

    expect(wrapper.find("#reset-pass").exists()).toBe(true);
    expect(wrapper.text()).toContain("permanently deletes all stored SSH keys");
    expect(wrapper.text()).toContain("This cannot be undone");
  });

  it("calls vault.reset with the entered passphrase and returns to setup", async () => {
    const vault = useVaultStore();
    const reset = vi.spyOn(vault, "reset").mockImplementation(async () => {
      vault.initialized = false;
      vault.unlocked = false;
    });

    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    await buttonWithText(wrapper, "Reset Vault").trigger("click");
    await flushPromises();

    await wrapper.get("#reset-pass").setValue("my-master-passphrase");
    await buttonWithText(wrapper, "Reset Vault").trigger("click");
    await flushPromises();

    expect(reset).toHaveBeenCalledWith("my-master-passphrase");
    expect(vault.initialized).toBe(false);
    expect(vault.unlocked).toBe(false);
    // After reset the setup form is shown, not the danger zone.
    expect(wrapper.find("#reset-pass").exists()).toBe(false);
    expect(wrapper.text()).toContain("Create Vault");
  });

  it("clears the entered passphrase on a failed reset and shows the error", async () => {
    const vault = useVaultStore();
    vi.spyOn(vault, "reset").mockRejectedValue(new Error("wrong passphrase"));

    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    await buttonWithText(wrapper, "Reset Vault").trigger("click");
    await flushPromises();

    await wrapper.get("#reset-pass").setValue("wrong");
    await buttonWithText(wrapper, "Reset Vault").trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("Error: wrong passphrase");
    // The passphrase must not linger in component state after a failure.
    expect((wrapper.get("#reset-pass").element as HTMLInputElement).value).toBe("");
    // The confirmation form stays open so the user can retry.
    expect(wrapper.find("#reset-pass").exists()).toBe(true);
  });

  it("cancels the reset form and clears the entered passphrase", async () => {
    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    await buttonWithText(wrapper, "Reset Vault").trigger("click");
    await flushPromises();

    await wrapper.get("#reset-pass").setValue("secret");
    await buttonWithText(wrapper, "Cancel").trigger("click");
    await flushPromises();

    expect(wrapper.find("#reset-pass").exists()).toBe(false);
    expect(wrapper.text()).toContain("Reset Vault");
  });

  it("clears the reset passphrase on a lock transition", async () => {
    const vault = useVaultStore();
    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    await buttonWithText(wrapper, "Reset Vault").trigger("click");
    await flushPromises();

    await wrapper.get("#reset-pass").setValue("secret");

    // Simulate an auto-lock while the reset form is open.
    vault.unlocked = false;
    await nextTick();

    const state = wrapper.vm as unknown as {
      resetPassphrase: string;
      resetMode: boolean;
    };
    expect(state.resetPassphrase).toBe("");
    expect(state.resetMode).toBe(false);
  });

  it("does not show the Danger Zone when the vault is locked", async () => {
    const vault = useVaultStore();
    vault.unlocked = false;

    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    expect(wrapper.text()).not.toContain("Danger Zone");
  });
});

describe("VaultView reset from locked screen", () => {
  let pinia: ReturnType<typeof createPinia>;
  let wrapper: ReturnType<typeof mount> | undefined;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    const vault = useVaultStore();
    vault.initialized = true;
    vault.unlocked = false;
    vault.biometricAvailable = false;
    vault.biometricEnabled = false;
    vi.spyOn(vault, "checkStatus").mockResolvedValue(undefined);
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = undefined;
    vi.restoreAllMocks();
  });

  it("shows a forgot-passphrase reset link on the unlock screen", async () => {
    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    expect(wrapper.text()).toContain("Forgot passphrase? Reset vault");
    // The destructive confirmation form is hidden until the link is used.
    expect(wrapper.find("#reset-pass").exists()).toBe(false);
  });

  it("reveals the reset confirmation form when the link is clicked", async () => {
    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    const link = wrapper.findAll("button").find((b) =>
      b.text().includes("Forgot passphrase"),
    )!;
    expect(link).toBeDefined();

    await link.trigger("click");
    await flushPromises();

    expect(wrapper.find("#reset-pass").exists()).toBe(true);
    expect(wrapper.text()).toContain("permanently deletes all stored SSH keys");
    // The unlock form is hidden while the reset form is open.
    expect(wrapper.find("#unlock-pass").exists()).toBe(false);
  });

  it("calls vault.reset from the locked flow and returns to setup", async () => {
    const vault = useVaultStore();
    const reset = vi.spyOn(vault, "reset").mockImplementation(async () => {
      vault.initialized = false;
      vault.unlocked = false;
    });

    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    const link = wrapper.findAll("button").find((b) =>
      b.text().includes("Forgot passphrase"),
    )!;
    await link.trigger("click");
    await flushPromises();

    await wrapper.get("#reset-pass").setValue("my-master-passphrase");
    await buttonWithText(wrapper, "Reset Vault").trigger("click");
    await flushPromises();

    expect(reset).toHaveBeenCalledWith("my-master-passphrase");
    expect(vault.initialized).toBe(false);
    expect(wrapper.text()).toContain("Create Vault");
  });

  it("cancels the locked reset flow and returns to the unlock form", async () => {
    wrapper = mount(VaultView, { global: { plugins: [pinia] } });
    await flushPromises();

    const link = wrapper.findAll("button").find((b) =>
      b.text().includes("Forgot passphrase"),
    )!;
    await link.trigger("click");
    await flushPromises();

    await buttonWithText(wrapper, "Cancel").trigger("click");
    await flushPromises();

    expect(wrapper.find("#reset-pass").exists()).toBe(false);
    expect(wrapper.find("#unlock-pass").exists()).toBe(true);
  });
});
