import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import TerminalPane from "./TerminalPane.vue";
import SftpBrowser from "./SftpBrowser.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useSettingsStore } from "../stores/settings";
import { setInvokeHandler } from "../test/setup";
import type { Host } from "../types";

const written = vi.hoisted(() => [] as string[]);
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80; rows = 24;
  textarea = document.createElement("textarea");
  loadAddon() {}
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div"); root.className = "xterm";
    root.append(this.textarea); container.append(root);
  }
  onData() { return { dispose() {} }; }
  focus() {}
  write(data: string | Uint8Array, callback?: () => void) {
    if (typeof data === "string") written.push(data);
    callback?.();
  }
  reset() {}
  dispose() { this.textarea.remove(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class {
  clearDecorations() {}
  findNext() { return false; }
  findPrevious() { return false; }
  onDidChangeResults() { return () => {}; }
} }));

const HOSTNAME = "secret-box.example.test";
const host: Host = {
  id: "atlas", label: "Atlas", hostname: HOSTNAME, port: 2222, username: "deploy",
  auth: "agent", tags: [],
};
const hostKeyError = `host key for ${HOSTNAME}:2222 changed`;
let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  written.length = 0;
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  setInvokeHandler("list_identities", () => []);
  useSettingsStore().maskAddresses = true;
});
afterEach(() => {
  wrapper?.unmount(); wrapper = undefined;
  vi.unstubAllGlobals();
});

describe("address masking in connection errors", () => {
  it("masks the host inside a terminal connection error", async () => {
    setInvokeHandler("connect_ssh", () => { throw hostKeyError; });
    useHostsStore().hosts = [host];
    const tab = useTabsStore().newTab(host);
    const pane = collectPanes(tab.tree)[0];
    wrapper = mount(TerminalPane, { props: { pane, tabId: tab.id, visible: true }, attachTo: document.body });

    await vi.waitFor(() => expect(written.join("")).toContain("Connection failed"));
    expect(written.join("")).not.toContain(HOSTNAME);
    expect(written.join("")).toContain("host key for se");
  });

  it("masks the host inside an SFTP connection error", async () => {
    setInvokeHandler("list_hosts", () => [host]);
    setInvokeHandler("sftp_connect", () => { throw hostKeyError; });
    wrapper = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
    await flushPromises();

    await wrapper.get("select").setValue(host.id);
    const connect = wrapper.findAll("button").find(button => button.text().includes("Connect"));
    await connect!.trigger("click");
    await vi.waitFor(() => expect(wrapper!.text()).toContain("host key for se"));
    expect(wrapper.text()).not.toContain(HOSTNAME);
  });

  it("masks the hostname a terminal attempt started with after the host is renamed", async () => {
    let rejectConnect: ((error: string) => void) | undefined;
    setInvokeHandler("connect_ssh", () => new Promise((_, reject) => { rejectConnect = reject; }));
    useHostsStore().hosts = [{ ...host }];
    const tab = useTabsStore().newTab(useHostsStore().hosts[0]);
    const pane = collectPanes(tab.tree)[0];
    wrapper = mount(TerminalPane, { props: { pane, tabId: tab.id, visible: true }, attachTo: document.body });

    await vi.waitFor(() => expect(rejectConnect).toBeDefined());
    useHostsStore().hosts[0].hostname = "renamed.example.test";
    rejectConnect!(hostKeyError);

    await vi.waitFor(() => expect(written.join("")).toContain("Connection failed"));
    expect(written.join("")).not.toContain(HOSTNAME);
  });

  it("masks the hostname an SFTP attempt started with after the host is renamed", async () => {
    let rejectConnect: ((error: string) => void) | undefined;
    setInvokeHandler("list_hosts", () => [{ ...host }]);
    setInvokeHandler("sftp_connect", () => new Promise((_, reject) => { rejectConnect = reject; }));
    wrapper = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
    await flushPromises();

    await wrapper.get("select").setValue(host.id);
    const connect = wrapper.findAll("button").find(button => button.text().includes("Connect"));
    await connect!.trigger("click");
    await vi.waitFor(() => expect(rejectConnect).toBeDefined());
    useHostsStore().hosts[0].hostname = "renamed.example.test";
    rejectConnect!(hostKeyError);

    await vi.waitFor(() => expect(wrapper!.text()).toContain("host key for se"));
    expect(wrapper.text()).not.toContain(HOSTNAME);
  });
});
