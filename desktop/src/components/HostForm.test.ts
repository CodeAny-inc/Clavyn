import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import HostForm from "./HostForm.vue";
import { useHostsStore } from "../stores/hosts";
import { useVaultStore } from "../stores/vault";
import { getInvokeMock, setInvokeHandler } from "../test/setup";

const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);

describe("HostForm", () => {
  let wrapper: VueWrapper;

  beforeEach(() => {
    setActivePinia(createPinia());
    getInvokeMock().mockClear();
    wrapper = mount(HostForm, {
      props: { host: null },
      global: { stubs: { Teleport: true } },
    });
  });

  afterEach(() => {
    if (wrapper.exists()) wrapper.unmount();
  });

  function addButton() {
    return wrapper.findAll("button").find((b) => b.text() === "Add host")!;
  }

  async function fillForm(port = "22") {
    await wrapper.get("#host-label").setValue("Web");
    await wrapper.get("#host-hostname").setValue("web.example.com");
    await wrapper.get("#host-username").setValue("deploy");
    await wrapper.get("#host-port").setValue(port);
  }

  it("adds exactly one host when Add host is clicked twice", async () => {
    let finish!: (host: unknown) => void;
    setInvokeHandler("add_host", ({ host }) => new Promise((resolve) => {
      finish = () => resolve(host);
    }));
    await fillForm();

    await addButton().trigger("click");
    await addButton().trigger("click");
    await flushPromises();

    expect(calls("add_host")).toHaveLength(1);
    expect(addButton().attributes("disabled")).toBeDefined();

    finish(undefined);
    await flushPromises();
    expect(useHostsStore().hosts).toHaveLength(1);
    expect(wrapper.emitted("close")).toHaveLength(1);
  });

  it.each(["70000", "0", "22.5", ""])("disables saving for port %j and says why", async (port) => {
    await fillForm(port);

    expect(addButton().attributes("disabled")).toBeDefined();
    expect(wrapper.text()).toContain("Port must be a whole number from 1 to 65535.");

    await addButton().trigger("click");
    await flushPromises();
    expect(calls("add_host")).toHaveLength(0);
  });

  it("accepts the highest valid port", async () => {
    await fillForm("65535");

    expect(addButton().attributes("disabled")).toBeUndefined();
    expect(wrapper.text()).not.toContain("Port must be a whole number");
  });

  it("shows the error and stays open when add_host fails", async () => {
    setInvokeHandler("add_host", () => {
      throw new Error("disk full");
    });
    await fillForm();

    await addButton().trigger("click");
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain("Could not save: Error: disk full");
    expect(wrapper.emitted("close")).toBeUndefined();
    expect(addButton().attributes("disabled")).toBeUndefined();
  });

  describe("key picker when the vault has no keys to list", () => {
    // Unlocking the vault loads its keys, so the backend has to answer.
    beforeEach(() => setInvokeHandler("list_keys", () => []));

    async function chooseKeyAuth() {
      const authSelect = wrapper
        .findAll("select")
        .find((s) => s.findAll("option").some((o) => o.attributes("value") === "publickey"))!;
      await authSelect.setValue("publickey");
    }

    it("says the vault could not be checked rather than that no keys exist", async () => {
      const vault = useVaultStore();
      vault.initialized = true;
      vault.statusUnavailable = true;

      await chooseKeyAuth();

      expect(wrapper.text()).toContain("The vault could not be checked");
      expect(wrapper.text()).not.toContain("No keys available");
    });

    it("still says no keys exist for a vault that was checked", async () => {
      const vault = useVaultStore();
      vault.initialized = true;
      vault.unlocked = true;

      await chooseKeyAuth();

      expect(wrapper.text()).toContain("No keys available");
      expect(wrapper.text()).not.toContain("The vault could not be checked");
    });
  });
});
