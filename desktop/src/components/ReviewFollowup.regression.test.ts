import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import SftpBrowser from "./SftpBrowser.vue";
import HostList from "./HostList.vue";
import WorkspaceView from "./WorkspaceView.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host, Identity, Workspace } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80; rows = 24;
  textarea = document.createElement("textarea");
  loadAddon() {}
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div"); root.className = "xterm";
    this.textarea.className = "xterm-helper-textarea";
    root.append(this.textarea); container.append(root);
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

const passwordMethod = { password: { credential_key: "fixture-only" } } as const;
const passwordHost = (id: string, label = id): Host => ({
  id, label, hostname: `${id}.example.test`, port: 22, username: "deploy",
  auth: passwordMethod, tags: [],
});
const agentHost = (id: string, label = id): Host => ({
  id, label, hostname: `${id}.example.test`, port: 22, username: "deploy",
  auth: "agent", tags: [],
});
const identity = (overrides: Partial<Identity> = {}): Identity => ({
  id: "shared", label: "Shared root", username: "root", auth: passwordMethod,
  tags: [], ...overrides,
});
const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  setInvokeHandler("list_groups", () => []);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("list_workspaces", () => []);
});
afterEach(() => {
  wrapper?.unmount(); wrapper = undefined;
  vi.unstubAllGlobals();
});

describe("credential ownership across persistent views", () => {
  it("cancels and clears an unsubmitted SSH password when its persistent terminal is hidden", async () => {
    const host = passwordHost("atlas", "Atlas");
    useHostsStore().hosts = [host];
    useTabsStore().newTab(host);
    wrapper = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });

    await vi.waitFor(() => expect(wrapper!.find('input[aria-label="SSH password"]').exists()).toBe(true));
    const input = wrapper.get<HTMLInputElement>('input[aria-label="SSH password"]');
    await input.setValue("DO_NOT_RETAIN");
    expect(input.element.value).toBe("DO_NOT_RETAIN");

    await wrapper.setProps({ visible: false });
    await flushPromises();
    expect(input.element.value).toBe("");
    expect(calls("connect_ssh")).toHaveLength(0);
    expect(wrapper.find('input[aria-label="SSH password"]').exists()).toBe(false);

    await wrapper.setProps({ visible: true });
    await vi.waitFor(() => expect(wrapper!.find('[role="alert"] button').exists()).toBe(true));
    await wrapper.get('[role="alert"] button').trigger("click");
    await vi.waitFor(() => expect(wrapper!.find('input[aria-label="SSH password"]').exists()).toBe(true));
    expect(wrapper.get<HTMLInputElement>('input[aria-label="SSH password"]').element.value).toBe("");
  });

  it("never carries an SFTP password from one selected host into another", async () => {
    const first = passwordHost("alpha", "Alpha");
    const middle = agentHost("beta", "Beta");
    const last = passwordHost("gamma", "Gamma");
    setInvokeHandler("list_hosts", () => [first, middle, last]);
    setInvokeHandler("sftp_canonicalize", () => "/home/gamma");
    setInvokeHandler("sftp_list_dir", () => []);
    wrapper = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
    await flushPromises();

    await wrapper.get("select").setValue(first.id);
    let input = wrapper.get<HTMLInputElement>('input[type="password"]');
    await input.setValue("ALPHA_SECRET");

    await wrapper.get("select").setValue(middle.id);
    expect(wrapper.find('input[type="password"]').exists()).toBe(false);
    await wrapper.get("select").setValue(last.id);
    input = wrapper.get<HTMLInputElement>('input[type="password"]');
    expect(input.element.value).toBe("");

    await input.setValue("GAMMA_SECRET");
    await wrapper.get("button:not([disabled])").trigger("click");
    await vi.waitFor(() => expect(calls("sftp_connect")).toHaveLength(1));
    const args = calls("sftp_connect")[0][1];
    expect(args.host.id).toBe("gamma");
    expect(args.password).toBe("GAMMA_SECRET");
    expect(JSON.stringify(args)).not.toContain("ALPHA_SECRET");
  });
});

describe("linked identity presentation and readiness", () => {
  it("keeps SFTP linked hosts fail-closed until the effective identity loads", async () => {
    const linked = agentHost("linked", "Linked"); linked.identity_id = "shared";
    const direct = agentHost("direct", "Direct");
    setInvokeHandler("list_hosts", () => [linked, direct]);
    setInvokeHandler("list_identities", () => { throw new Error("fixture identity outage"); });
    wrapper = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
    await flushPromises();

    await wrapper.get("select").setValue(linked.id);
    expect(wrapper.get<HTMLButtonElement>('button').element.disabled).toBe(false); // Retry button remains usable.
    const buttons = wrapper.findAll<HTMLButtonElement>("button");
    const connect = buttons.find(button => button.text().includes("Connect"));
    expect(connect).toBeDefined();
    expect(connect!.element.disabled).toBe(true);
    expect(wrapper.find('input[type="password"]').exists()).toBe(false);
    expect(wrapper.text()).not.toContain("deploy@linked.example.test");

    setInvokeHandler("list_identities", () => [identity()]);
    const retry = buttons.find(button => button.text().includes("Retry identities"));
    expect(retry).toBeDefined();
    await retry!.trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("root@linked.example.test:22");
    expect(wrapper.find('input[type="password"]').exists()).toBe(true);

    await wrapper.get("select").setValue(direct.id);
    const directConnect = wrapper.findAll<HTMLButtonElement>("button").find(button => button.text().includes("Connect"));
    expect(directConnect?.element.disabled).toBe(false);
  });

  it("HostList displays and searches the resolved identity username instead of the host fallback", async () => {
    const linked = agentHost("atlas", "Atlas"); linked.identity_id = "shared";
    setInvokeHandler("list_hosts", () => [linked]);
    setInvokeHandler("list_identities", () => [identity({ auth: "agent" })]);
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();

    expect(wrapper.text()).toContain("root@atlas.example.test:22");
    expect(wrapper.text()).not.toContain("deploy@atlas.example.test:22");
    const search = wrapper.get('input[placeholder="Search hosts..."]');
    await search.setValue("root");
    expect(wrapper.text()).toContain("Atlas");
    await search.setValue("deploy");
    expect(wrapper.text()).toContain("No hosts yet");
  });
});

describe("workspace restore connection policy", () => {
  it.each([
    [false, 0],
    [true, 1],
  ] as const)("restores SSH panes with auto_connect=%s and starts %i connection(s)", async (autoConnect, expected) => {
    const host = agentHost("atlas", "Atlas");
    const workspace: Workspace = {
      id: "workspace-1", name: "Workspace", description: null, color: null, icon: null,
      host_ids: [host.id], auto_connect: autoConnect,
      tabs: [{ id: "workspace-tab", title: "Atlas tab", layout: {
        type: "pane", host_id: host.id, terminal_type: "ssh",
      } }],
    };
    setInvokeHandler("list_hosts", () => [host]);
    setInvokeHandler("list_workspaces", () => [workspace]);
    wrapper = mount(WorkspaceView, { attachTo: document.body });
    await flushPromises();
    await wrapper.get('[aria-label="Restore workspace"]').trigger("click");
    await flushPromises();

    const pane = collectPanes(useTabsStore().tabs[0].tree)[0];
    expect(pane.autoConnect).toBe(autoConnect);
    wrapper.unmount();
    wrapper = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });
    await flushPromises();
    if (expected) await vi.waitFor(() => expect(calls("connect_ssh")).toHaveLength(expected));
    else expect(calls("connect_ssh")).toHaveLength(0);
  });
});
