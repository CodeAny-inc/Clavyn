import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import VaultUnlockModal from "./VaultUnlockModal.vue";
import { useUiStore } from "../stores/ui";
import { useVaultStore } from "../stores/vault";

describe("VaultUnlockModal", () => {
  let wrapper: VueWrapper;

  beforeEach(() => {
    const pinia = createPinia();
    setActivePinia(pinia);
    wrapper = mount(VaultUnlockModal, {
      global: {
        plugins: [pinia],
        stubs: {
          Teleport: true,
        },
      },
    });
  });

  afterEach(() => {
    wrapper.unmount();
  });

  it("is a named modal dialog, so app shortcuts leave it alone", async () => {
    vi.spyOn(useVaultStore(), "refreshBiometricState").mockResolvedValue(undefined);
    void useUiStore().requestVaultUnlock().catch(() => {});
    await flushPromises();

    const dialog = wrapper.get('[role="dialog"]');
    expect(dialog.attributes("aria-modal")).toBe("true");
    const title = wrapper.get(`#${dialog.attributes("aria-labelledby")}`);
    expect(title.text()).toBe("Unlock Vault");
  });

  it("closes on Escape wherever focus is inside it", async () => {
    vi.spyOn(useVaultStore(), "refreshBiometricState").mockResolvedValue(undefined);
    const ui = useUiStore();
    const request = ui.requestVaultUnlock().catch(() => "cancelled");
    await flushPromises();

    // Taken at the document, so it works from any control in the prompt and
    // even when focus has fallen out of it.
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flushPromises();
    expect(ui.showVaultUnlockModal).toBe(false);
    await request;
  });

  it("offers Touch ID for the SSH unlock flow and resolves on success", async () => {
    const vault = useVaultStore();
    const ui = useUiStore();
    vault.biometricAvailable = true;
    vault.biometricEnabled = true;
    vi.spyOn(vault, "refreshBiometricState").mockResolvedValue(undefined);

    const unlockWithBiometric = vi
      .spyOn(vault, "unlockWithBiometric")
      .mockResolvedValue(undefined);

    const unlockRequest = ui.requestVaultUnlock();
    await flushPromises();

    const touchIdButton = wrapper
      .findAll("button")
      .find((button) => button.text().includes("Unlock with Touch ID"));

    expect(touchIdButton).toBeDefined();
    await touchIdButton!.trigger("click");

    expect(unlockWithBiometric).toHaveBeenCalledTimes(1);
    await expect(unlockRequest).resolves.toBe(true);
    expect(ui.showVaultUnlockModal).toBe(false);
  });

  it("blocks duplicate actions and cancellation while Touch ID is pending", async () => {
    const vault = useVaultStore();
    const ui = useUiStore();
    vault.biometricAvailable = true;
    vault.biometricEnabled = true;
    vi.spyOn(vault, "refreshBiometricState").mockResolvedValue(undefined);

    let finishBiometric!: () => void;
    const unlockWithBiometric = vi
      .spyOn(vault, "unlockWithBiometric")
      .mockImplementation(
        () =>
          new Promise<void>((resolve) => {
            finishBiometric = resolve;
          }),
      );

    const unlockRequest = ui.requestVaultUnlock();
    await flushPromises();

    const touchIdButton = wrapper
      .findAll("button")
      .find((button) => button.text().includes("Unlock with Touch ID"));
    expect(touchIdButton).toBeDefined();

    await touchIdButton!.trigger("click");
    await nextTick();

    expect(unlockWithBiometric).toHaveBeenCalledTimes(1);

    const pendingTouchIdButton = wrapper
      .findAll("button")
      .find((button) => button.text().includes("Waiting for Touch ID"));
    expect(pendingTouchIdButton).toBeDefined();
    expect((pendingTouchIdButton!.element as HTMLButtonElement).disabled).toBe(true);
    expect((wrapper.get("#modal-pass").element as HTMLInputElement).disabled).toBe(true);

    const cancelButton = wrapper
      .findAll("button")
      .find((button) => button.text().trim() === "Cancel");
    const passwordUnlockButton = wrapper
      .findAll("button")
      .find((button) => button.text().trim() === "Unlock");

    expect((cancelButton!.element as HTMLButtonElement).disabled).toBe(true);
    expect((passwordUnlockButton!.element as HTMLButtonElement).disabled).toBe(true);

    await pendingTouchIdButton!.trigger("click");
    expect(unlockWithBiometric).toHaveBeenCalledTimes(1);
    expect(ui.showVaultUnlockModal).toBe(true);

    finishBiometric();
    await flushPromises();

    await expect(unlockRequest).resolves.toBe(true);
    expect(ui.showVaultUnlockModal).toBe(false);
  });

  it("refreshes Touch ID availability every time the SSH unlock modal opens", async () => {
    const vault = useVaultStore();
    const ui = useUiStore();
    vault.initialized = true;
    vault.biometricAvailable = false;
    vault.biometricEnabled = true;

    const refreshBiometricState = vi
      .spyOn(vault, "refreshBiometricState")
      .mockImplementation(async () => {
        vault.biometricAvailable = true;
        vault.biometricEnabled = true;
      });

    const unlockRequest = ui.requestVaultUnlock();
    await flushPromises();

    expect(refreshBiometricState).toHaveBeenCalledTimes(1);
    const touchIdButton = wrapper
      .findAll("button")
      .find((button) => button.text().includes("Unlock with Touch ID"));
    expect(touchIdButton).toBeDefined();

    ui.resolveVaultUnlock(false);
    await expect(unlockRequest).resolves.toBe(false);
  });

  it("explains a rolled-back vault and opens the older copy only on request", async () => {
    const vault = useVaultStore();
    const ui = useUiStore();
    vi.spyOn(vault, "refreshBiometricState").mockResolvedValue(undefined);
    const unlock = vi
      .spyOn(vault, "unlock")
      .mockRejectedValueOnce("[vault-rollback] This vault file is older than one this device has already opened.")
      .mockResolvedValueOnce(undefined);

    const unlockRequest = ui.requestVaultUnlock();
    await flushPromises();
    await wrapper.get("#modal-pass").setValue("passphrase");
    await wrapper.findAll("button").find((b) => b.text() === "Unlock")!.trigger("click");
    await flushPromises();

    expect(unlock).toHaveBeenLastCalledWith("passphrase", false);
    const notice = wrapper.get('[data-testid="vault-rollback"]');
    expect(notice.text()).toContain("older than one this device has already opened");
    expect(notice.text()).not.toContain("[vault-rollback]");
    expect(ui.showVaultUnlockModal).toBe(true);

    await notice.get("button").trigger("click");
    await flushPromises();

    expect(unlock).toHaveBeenLastCalledWith("passphrase", true);
    await expect(unlockRequest).resolves.toBe(true);
  });
});
