import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import IdentityManager from "./IdentityManager.vue";
import { useVaultStore } from "../stores/vault";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Identity } from "../types";

const PRIVATE_KEY =
  "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";

const NEW_KEY = {
  id: "key-new",
  label: "laptop",
  key_type: "ed25519",
  fingerprint: "SHA256:new",
  public_key_base64: "AAAA",
};

const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);

type KeyRefs = { importKeyText: string; importKeyPassphrase: string };

describe("IdentityManager", () => {
  let wrapper: VueWrapper;

  beforeEach(() => {
    setActivePinia(createPinia());
    setInvokeHandler("list_identities", () => []);
    setInvokeHandler("list_hosts", () => []);
    setInvokeHandler("list_groups", () => []);
    setInvokeHandler("list_keys", () => []);
    wrapper = mount(IdentityManager, {
      global: { stubs: { Teleport: true } },
    });
  });

  afterEach(() => {
    if (wrapper.exists()) wrapper.unmount();
    vi.restoreAllMocks();
  });

  function button(text: string) {
    const found = wrapper.findAll("button").find((b) => b.text() === text);
    expect(found, `Missing the "${text}" button`).toBeDefined();
    return found!;
  }

  function refs() {
    return wrapper.vm as unknown as KeyRefs;
  }

  async function openKeyTab(tab: "Generate new" | "Import") {
    await button("Add Identity").trigger("click");
    await wrapper.get("#id-label").setValue("Work");
    await wrapper.get("#id-username").setValue("deploy");
    const authSelect = wrapper
      .findAll("select")
      .find((s) => s.findAll("option").some((o) => o.attributes("value") === "publickey"))!;
    await authSelect.setValue("publickey");
    await button(tab).trigger("click");
  }

  async function openImportWithKeyMaterial() {
    await openKeyTab("Import");
    await wrapper.get("#imp-key-label").setValue("laptop");
    await wrapper.get("#imp-key-private").setValue(PRIVATE_KEY);
    await wrapper.get("#imp-key-pass").setValue("hunter2");
    expect(refs().importKeyText).toBe(PRIVATE_KEY);
  }

  describe("saving", () => {
    it("creates one key and one identity when Add identity is clicked twice", async () => {
      // The key is generated first; the second click lands while the identity
      // write that follows it is still in flight.
      setInvokeHandler("generate_key", () => NEW_KEY);
      let finish!: () => void;
      setInvokeHandler("add_identity", ({ identity }) => new Promise((resolve) => {
        finish = () => resolve(identity);
      }));
      await openKeyTab("Generate new");
      await wrapper.get("#gen-key-label").setValue("laptop");

      await button("Add identity").trigger("click");
      await flushPromises();
      expect(calls("add_identity")).toHaveLength(1);
      await button("Add identity").trigger("click");
      await flushPromises();

      finish();
      await flushPromises();

      expect(calls("generate_key")).toHaveLength(1);
      expect(calls("add_identity")).toHaveLength(1);
    });

    it("links the key it already created when retrying a failed identity save", async () => {
      setInvokeHandler("generate_key", () => NEW_KEY);
      let attempts = 0;
      setInvokeHandler("add_identity", ({ identity }) => {
        attempts += 1;
        if (attempts === 1) throw new Error("disk full");
        return identity;
      });
      await openKeyTab("Generate new");
      await wrapper.get("#gen-key-label").setValue("laptop");

      await button("Add identity").trigger("click");
      await flushPromises();

      expect(wrapper.get('[role="alert"]').text()).toContain("Failed to save identity: Error: disk full");
      expect(wrapper.find("#id-label").exists()).toBe(true);

      await button("Add identity").trigger("click");
      await flushPromises();

      expect(calls("generate_key")).toHaveLength(1);
      expect(calls("add_identity")).toHaveLength(2);
      expect((calls("add_identity")[1][1].identity as Identity).key_id).toBe("key-new");
      expect(wrapper.find("#id-label").exists()).toBe(false);
    });

    it("shows the error when deleting an identity fails", async () => {
      setInvokeHandler("list_identities", () => [
        { id: "id-1", label: "Work", username: "deploy", auth: "publickey", key_id: null, tags: [], group_id: null },
      ]);
      setInvokeHandler("delete_identity", () => {
        throw new Error("store locked");
      });
      wrapper.unmount();
      wrapper = mount(IdentityManager, { global: { stubs: { Teleport: true } } });
      await flushPromises();

      await wrapper.get('[aria-label="Delete identity"]').trigger("click");
      await flushPromises();

      expect(wrapper.get('[role="alert"]').text()).toContain("Failed to delete identity: Error: store locked");
      expect(wrapper.text()).toContain("Work");
    });
  });

  describe("imported key material", () => {
    it("is dropped after a successful save", async () => {
      setInvokeHandler("import_key", () => NEW_KEY);
      setInvokeHandler("add_identity", ({ identity }) => identity);
      await openImportWithKeyMaterial();

      await button("Add identity").trigger("click");
      await flushPromises();

      expect(calls("add_identity")).toHaveLength(1);
      expect(refs().importKeyText).toBe("");
      expect(refs().importKeyPassphrase).toBe("");
    });

    it("is dropped when the dialog is cancelled", async () => {
      await openImportWithKeyMaterial();

      await button("Cancel").trigger("click");

      expect(refs().importKeyText).toBe("");
      expect(refs().importKeyPassphrase).toBe("");
    });

    it("is dropped when the import is rejected, keeping the dialog and the label", async () => {
      setInvokeHandler("import_key", () => {
        throw new Error("wrong passphrase");
      });
      await openImportWithKeyMaterial();

      await button("Add identity").trigger("click");
      await flushPromises();

      expect(calls("add_identity")).toHaveLength(0);
      expect(refs().importKeyText).toBe("");
      expect(refs().importKeyPassphrase).toBe("");
      expect((wrapper.get("#imp-key-private").element as HTMLTextAreaElement).value).toBe("");
      expect((wrapper.get("#imp-key-label").element as HTMLInputElement).value).toBe("laptop");
      expect(wrapper.get('[role="alert"]').text()).toContain("Failed to import key: Error: wrong passphrase");
    });

    it("is dropped and the dialog closed when the vault locks", async () => {
      const vault = useVaultStore();
      vault.unlocked = true;
      await flushPromises();
      await openImportWithKeyMaterial();

      vault.unlocked = false;
      await flushPromises();

      expect(refs().importKeyText).toBe("");
      expect(refs().importKeyPassphrase).toBe("");
      expect(wrapper.find("#imp-key-private").exists()).toBe(false);
      expect(wrapper.html()).not.toContain("b3BlbnNzaC1rZXktdjEAAAAA");
    });

    it("is dropped when the view is unmounted", async () => {
      await openImportWithKeyMaterial();
      const vm = refs();

      wrapper.unmount();

      expect(vm.importKeyText).toBe("");
      expect(vm.importKeyPassphrase).toBe("");
    });
  });
});
