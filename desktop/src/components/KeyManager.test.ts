import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import KeyManager from "./KeyManager.vue";
import { useKeysStore } from "../stores/keys";
import { useVaultStore } from "../stores/vault";
import { setInvokeHandler } from "../test/setup";

const PRIVATE_KEY =
  "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";

describe("KeyManager add-key dialog", () => {
  let wrapper: VueWrapper;

  beforeEach(() => {
    setActivePinia(createPinia());
    setInvokeHandler("list_keys", () => []);
    vi.stubGlobal("alert", vi.fn());
    wrapper = mount(KeyManager, {
      global: { stubs: { Teleport: true } },
    });
  });

  afterEach(() => {
    if (wrapper.exists()) wrapper.unmount();
    vi.unstubAllGlobals();
  });

  async function openImportTabWithKeyMaterial() {
    await wrapper.findAll("button").find((b) => b.text().includes("Add Key"))!.trigger("click");
    await wrapper.findAll("button").find((b) => b.text() === "Import")!.trigger("click");
    await wrapper.get("#key-label").setValue("laptop");
    await wrapper.get("#key-private").setValue(PRIVATE_KEY);
    await wrapper.get("#key-passphrase").setValue("hunter2");
    expect((wrapper.get("#key-private").element as HTMLTextAreaElement).value).toBe(PRIVATE_KEY);
  }

  async function reopenImportTab() {
    await wrapper.findAll("button").find((b) => b.text().includes("Add Key"))!.trigger("click");
    await wrapper.findAll("button").find((b) => b.text() === "Import")!.trigger("click");
  }

  it("drops the private key and passphrase when the dialog is cancelled", async () => {
    await openImportTabWithKeyMaterial();

    await wrapper.findAll("button").find((b) => b.text() === "Cancel")!.trigger("click");
    await reopenImportTab();

    expect((wrapper.get("#key-private").element as HTMLTextAreaElement).value).toBe("");
    expect((wrapper.get("#key-passphrase").element as HTMLInputElement).value).toBe("");
  });

  it("drops the private key and passphrase when the dialog is dismissed from its overlay", async () => {
    await openImportTabWithKeyMaterial();

    wrapper.findComponent({ name: "Dialog" }).vm.$emit("close");
    await flushPromises();
    await reopenImportTab();

    expect((wrapper.get("#key-private").element as HTMLTextAreaElement).value).toBe("");
    expect((wrapper.get("#key-passphrase").element as HTMLInputElement).value).toBe("");
  });

  it("drops the private key and passphrase when the import is rejected, keeping the dialog and the label", async () => {
    const keys = useKeysStore();
    const importKey = vi
      .spyOn(keys, "importKey")
      .mockRejectedValue(new Error("unsupported key format"));

    await openImportTabWithKeyMaterial();
    await wrapper.findAll("button").find((b) => b.text() === "Import Key")!.trigger("click");
    await flushPromises();

    expect(importKey).toHaveBeenCalledTimes(1);

    // The dialog is still open on the Import tab, so the fields are reachable
    // without reopening anything.
    expect((wrapper.get("#key-private").element as HTMLTextAreaElement).value).toBe("");
    expect((wrapper.get("#key-passphrase").element as HTMLInputElement).value).toBe("");
    expect((wrapper.get("#key-label").element as HTMLInputElement).value).toBe("laptop");
    expect(wrapper.findAll("button").some((b) => b.text() === "Import Key")).toBe(true);
  });

  it("closes the dialog and drops the private key and passphrase when the vault locks", async () => {
    const vault = useVaultStore();
    vault.unlocked = true;
    await flushPromises();

    await openImportTabWithKeyMaterial();

    // What an auto-lock actually does: `useAutoLock` calls `vault.lock()` and
    // touches nothing else. No unlock modal opens, and this view stays mounted.
    vault.unlocked = false;
    await flushPromises();

    const addForm = (wrapper.vm as unknown as { addForm: Record<string, string> }).addForm;
    expect(addForm.privateKey).toBe("");
    expect(addForm.passphrase).toBe("");
    expect(wrapper.find("#key-private").exists()).toBe(false);
    expect(wrapper.html()).not.toContain("b3BlbnNzaC1rZXktdjEAAAAA");
  });

  it("keeps the dialog and the typed form when the vault unlocks", async () => {
    const vault = useVaultStore();
    expect(vault.unlocked).toBe(false);

    // This view is reachable while the vault is locked, so a user can already
    // be mid-import when the unlock lands. An unlock only ever adds authority,
    // and it cannot change who is at the keyboard: reaching it with key
    // material still in the form means the same person typed it and then
    // authenticated, with no lock in between to contain.
    await openImportTabWithKeyMaterial();

    vault.unlocked = true;
    await flushPromises();

    expect((wrapper.get("#key-private").element as HTMLTextAreaElement).value).toBe(PRIVATE_KEY);
    expect((wrapper.get("#key-passphrase").element as HTMLInputElement).value).toBe("hunter2");
    expect((wrapper.get("#key-label").element as HTMLInputElement).value).toBe("laptop");
  });

  it("drops the private key and passphrase when the view is unmounted", async () => {
    await openImportTabWithKeyMaterial();
    const addForm = (wrapper.vm as unknown as { addForm: Record<string, string> }).addForm;
    expect(addForm.privateKey).toBe(PRIVATE_KEY);

    wrapper.unmount();

    expect(addForm.privateKey).toBe("");
    expect(addForm.passphrase).toBe("");
  });
});
