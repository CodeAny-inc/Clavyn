import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import HostForm from "./HostForm.vue";
import IdentityManager from "./IdentityManager.vue";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Identity } from "../types";

// The backend refuses a host or identity write whose native confirmation was
// cancelled, and tags the error so the form can tell it from a failure.
const DECLINED = "[host-change-confirmation-declined] Allow a vault key to be used: cancelled";

const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
let wrapper: VueWrapper | undefined;

function keyIdentity(): Identity {
  return {
    id: "ops-identity", label: "Ops", username: "root",
    auth: "publickey", key_id: "key-1", tags: [], group_id: null,
  };
}

// Dialogs teleport to document.body, so their contents are queried there.
function input(selector: string, value: string) {
  const field = document.querySelector<HTMLInputElement>(selector)!;
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

function button(text: string) {
  return Array.from(document.querySelectorAll<HTMLButtonElement>("button"))
    .find(candidate => candidate.textContent?.includes(text));
}

function alertText() {
  return document.querySelector('[role="alert"]')?.textContent ?? "";
}

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  setInvokeHandler("list_groups", () => []);
  setInvokeHandler("list_hosts", () => []);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("list_keys", () => []);
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  vi.restoreAllMocks();
});

describe("HostForm when a save is not confirmed", () => {
  async function fillNewHost() {
    wrapper = mount(HostForm, { props: { host: null }, attachTo: document.body });
    input("#host-label", "Prod");
    input("#host-hostname", "prod.example.test");
    input("#host-username", "deploy");
    await flushPromises();
  }

  it("keeps the form open, says the change was not saved, and saves on a confirmed retry", async () => {
    let answer: "decline" | "allow" = "decline";
    setInvokeHandler("add_host", ({ host }) => {
      if (answer === "decline") return Promise.reject(DECLINED);
      return host;
    });
    await fillNewHost();

    button("Add host")!.click();
    await vi.waitFor(() => expect(alertText()).toContain("Not saved"));
    expect(alertText()).toContain("not allowed in the confirmation dialog");
    expect(alertText()).not.toContain("[host-change-confirmation-declined]");
    expect(wrapper!.emitted("close")).toBeUndefined();
    expect(useHostsStore().hosts).toHaveLength(0);
    expect((document.querySelector("#host-hostname") as HTMLInputElement).value).toBe("prod.example.test");

    answer = "allow";
    button("Add host")!.click();
    await vi.waitFor(() => expect(wrapper!.emitted("close")).toHaveLength(1));
    expect(calls("add_host")).toHaveLength(2);
    expect(useHostsStore().hosts.map(host => host.hostname)).toEqual(["prod.example.test"]);
  });

  it("reports a failure that is not a cancellation as one", async () => {
    setInvokeHandler("add_host", () => Promise.reject("store.json was not written"));
    await fillNewHost();

    button("Add host")!.click();
    await vi.waitFor(() => expect(alertText()).toContain("Could not save: store.json was not written"));
    expect(wrapper!.emitted("close")).toBeUndefined();
  });

  it("sends one write while the confirmation is still open", async () => {
    let answer: (value: unknown) => void = () => {};
    setInvokeHandler("add_host", ({ host }) => new Promise(resolve => { answer = () => resolve(host); }));
    await fillNewHost();

    button("Add host")!.click();
    await flushPromises();
    expect(button("Add host")!.disabled).toBe(true);
    button("Add host")!.click();
    await flushPromises();
    expect(calls("add_host")).toHaveLength(1);

    answer(undefined);
    await vi.waitFor(() => expect(wrapper!.emitted("close")).toHaveLength(1));
  });
});

describe("IdentityManager when a save is not confirmed", () => {
  it("keeps the edit dialog open with the change, and does not alert", async () => {
    const identity = keyIdentity();
    setInvokeHandler("list_identities", () => [identity]);
    setInvokeHandler("list_keys", () => [{ id: "key-1", label: "laptop", key_type: "ed25519", fingerprint: "SHA256:x", public_key_base64: "" }]);
    setInvokeHandler("update_identity", () => Promise.reject(DECLINED));
    const alert = vi.spyOn(window, "alert");

    wrapper = mount(IdentityManager, { attachTo: document.body });
    await flushPromises();
    await wrapper.get('[aria-label="Edit identity"]').trigger("click");
    await flushPromises();
    input("#id-username", "admin");
    await flushPromises();

    button("Save changes")!.click();
    await vi.waitFor(() => expect(alertText()).toContain("Not saved"));
    expect(alert).not.toHaveBeenCalled();
    expect(button("Save changes")).toBeDefined();
    expect((document.querySelector("#id-username") as HTMLInputElement).value).toBe("admin");
    expect(useIdentitiesStore().identities[0].username).toBe("root");
  });

  it("reuses a key it just generated when the save is retried", async () => {
    setInvokeHandler("generate_key", ({ label }) => ({
      id: "generated-key", label, key_type: "ed25519", fingerprint: "SHA256:g", public_key_base64: "",
    }));
    let answer: "decline" | "allow" = "decline";
    setInvokeHandler("add_identity", ({ identity }) => {
      if (answer === "decline") return Promise.reject(DECLINED);
      return identity;
    });

    wrapper = mount(IdentityManager, { attachTo: document.body });
    await flushPromises();
    button("Add Identity")!.click();
    await flushPromises();
    input("#id-label", "Deploy");
    input("#id-username", "deploy");
    const auth = Array.from(document.querySelectorAll<HTMLSelectElement>("select"))
      .find(select => Array.from(select.options).some(option => option.value === "publickey"))!;
    auth.value = "publickey";
    auth.dispatchEvent(new Event("change", { bubbles: true }));
    await flushPromises();
    button("Generate new")!.click();
    await flushPromises();
    input("#gen-key-label", "deploy key");
    await flushPromises();

    const dialogSave = () => button("Add identity")!;
    dialogSave().click();
    await vi.waitFor(() => expect(alertText()).toContain("Not saved"));
    expect(calls("generate_key")).toHaveLength(1);

    answer = "allow";
    dialogSave().click();
    await vi.waitFor(() => expect(calls("add_identity")).toHaveLength(2));
    expect(calls("generate_key")).toHaveLength(1);
    expect(calls("add_identity")[1][1].identity.key_id).toBe("generated-key");
  });

  it("stays quiet when deleting is cancelled in the native dialog, and reports other failures", async () => {
    const identity = keyIdentity();
    setInvokeHandler("list_identities", () => [identity]);
    let failure: unknown = DECLINED;
    setInvokeHandler("delete_identity", () => Promise.reject(failure));
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const alert = vi.spyOn(window, "alert");

    wrapper = mount(IdentityManager, { attachTo: document.body });
    await flushPromises();
    await wrapper.get('[aria-label="Delete identity"]').trigger("click");
    await flushPromises();
    expect(calls("delete_identity")).toHaveLength(1);
    expect(alert).not.toHaveBeenCalled();
    expect(useIdentitiesStore().identities).toHaveLength(1);

    failure = "store.json was not written";
    await wrapper.get('[aria-label="Delete identity"]').trigger("click");
    await flushPromises();
    expect(alert).toHaveBeenCalledWith("Failed to delete identity: store.json was not written");
  });
});
