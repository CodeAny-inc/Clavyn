import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import SftpBrowser from "./SftpBrowser.vue";
import HostForm from "./HostForm.vue";
import IdentityManager from "./IdentityManager.vue";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { useTabsStore } from "../stores/tabs";
import { useUiStore } from "../stores/ui";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host, Identity } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80; rows = 24;
  textarea = document.createElement("textarea");
  loadAddon() {}
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div");
    root.className = "xterm";
    this.textarea.className = "xterm-helper-textarea";
    root.append(this.textarea);
    container.append(root);
  }
  onData() { return { dispose() {} }; }
  focus() { this.textarea.focus(); }
  write(_data: string | Uint8Array, callback?: () => void) { callback?.(); }
  reset() {}
  dispose() { this.textarea.remove(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class {
  clearDecorations() {}
  findNext() { return false; }
  findPrevious() { return false; }
} }));

const passwordMethod = { password: { credential_key: "legacy" } } as const;
const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
let wrapper: VueWrapper | undefined;

function linkedMissingHost(overrides: Partial<Host> = {}): Host {
  return {
    id: "linked", label: "Linked", hostname: "linked.example.test", port: 22,
    username: "stale-user", auth: passwordMethod, identity_id: "deleted-identity",
    key_id: null, tags: [], ...overrides,
  };
}

function agentHost(): Host {
  return {
    id: "agent-host", label: "Legacy Agent", hostname: "agent.example.test", port: 22,
    username: "deploy", auth: "agent", identity_id: null, key_id: null, tags: [],
  };
}

function agentIdentity(): Identity {
  return {
    id: "agent-identity", label: "Legacy Agent Identity", username: "root",
    auth: "agent", key_id: null, tags: [], group_id: null,
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  setInvokeHandler("list_groups", () => []);
  setInvokeHandler("list_hosts", () => []);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("list_keys", () => []);
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  vi.unstubAllGlobals();
});

describe("missing linked identity credential preflight", () => {
  it("Terminal rejects the broken reference before password or vault UI", async () => {
    const host = linkedMissingHost();
    useHostsStore().hosts = [host];
    const identities = useIdentitiesStore();
    identities.identities = [];
    identities.loaded = true;
    useTabsStore().newTab(host);

    wrapper = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });
    await vi.waitFor(() => expect(wrapper!.text()).toContain("Linked SSH identity not found"));

    expect(wrapper.find('input[aria-label="SSH password"]').exists()).toBe(false);
    expect(useUiStore().showVaultUnlockModal).toBe(false);
    expect(calls("connect_ssh")).toHaveLength(0);
  });

  it("SFTP disables a broken linked host without exposing the stale password field", async () => {
    const host = linkedMissingHost();
    setInvokeHandler("list_hosts", () => [host]);
    setInvokeHandler("list_identities", () => []);

    wrapper = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
    await flushPromises();
    await wrapper.get("select").setValue(host.id);
    await flushPromises();

    expect(wrapper.text()).toContain("Missing SSH identity");
    expect(wrapper.find('input[type="password"]').exists()).toBe(false);
    const connect = wrapper.findAll<HTMLButtonElement>("button").find(button => button.text().includes("Connect"));
    expect(connect).toBeDefined();
    expect(connect!.element.disabled).toBe(true);
    expect(calls("sftp_connect")).toHaveLength(0);
  });
});

describe("legacy Agent edit preservation", () => {
  it("HostForm preserves Agent auth when saving an unrelated host edit", async () => {
    const host = agentHost();
    useHostsStore().hosts = [host];
    setInvokeHandler("update_host", ({ host: saved }) => saved);

    wrapper = mount(HostForm, { props: { host }, attachTo: document.body });
    const authSelect = wrapper.findAll<HTMLSelectElement>("select").find(select => select.element.value === "agent");
    expect(authSelect).toBeDefined();
    expect(authSelect!.text()).toContain("SSH Agent (unsupported)");

    await wrapper.get('#host-label').setValue("Renamed Agent Host");
    const save = wrapper.findAll("button").find(button => button.text().includes("Save changes"));
    expect(save).toBeDefined();
    await save!.trigger("click");
    await vi.waitFor(() => expect(calls("update_host")).toHaveLength(1));

    expect(calls("update_host")[0][1].host.auth).toBe("agent");
    expect(calls("update_host")[0][1].host.label).toBe("Renamed Agent Host");
  });

  it("IdentityManager preserves Agent auth when saving an unrelated identity edit", async () => {
    const identity = agentIdentity();
    setInvokeHandler("list_identities", () => [identity]);
    setInvokeHandler("update_identity", ({ identity: saved }) => saved);

    wrapper = mount(IdentityManager, { attachTo: document.body });
    await flushPromises();
    await wrapper.get('[aria-label="Edit identity"]').trigger("click");

    const authSelect = wrapper.findAll<HTMLSelectElement>("select").find(select => select.element.value === "agent");
    expect(authSelect).toBeDefined();
    expect(authSelect!.text()).toContain("SSH Agent (unsupported)");

    await wrapper.get('#id-label').setValue("Renamed Agent Identity");
    const save = wrapper.findAll("button").find(button => button.text().includes("Save changes"));
    expect(save).toBeDefined();
    await save!.trigger("click");
    await vi.waitFor(() => expect(calls("update_identity")).toHaveLength(1));

    expect(calls("update_identity")[0][1].identity.auth).toBe("agent");
    expect(calls("update_identity")[0][1].identity.label).toBe("Renamed Agent Identity");
  });
});
