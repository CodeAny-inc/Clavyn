import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { listen } from "@tauri-apps/api/event";
import SftpBrowser from "./SftpBrowser.vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useSftpStore } from "../stores/sftp";
import { useVaultStore } from "../stores/vault";
import { emitTauriEvent, getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const terminals = vi.hoisted(() => ({ items: [] as Array<{
  output: string[];
  input: (data: string) => void;
}> }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80; rows = 24;
  textarea = document.createElement("textarea");
  output: string[] = [];
  input: (data: string) => void = () => {};
  constructor() { terminals.items.push(this); }
  loadAddon() {}
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div"); root.className = "xterm";
    root.append(this.textarea); container.append(root);
  }
  onData(handler: (data: string) => void) { this.input = handler; }
  focus() { this.textarea.focus(); }
  write(data: string | Uint8Array, callback?: () => void) {
    this.output.push(typeof data === "string" ? data : new TextDecoder().decode(data));
    callback?.();
  }
  reset() { this.output.length = 0; }
  dispose() { this.textarea.remove(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class {
  clearDecorations() {}
  findNext() { return false; }
  findPrevious() { return false; }
} }));

const passwordHost: Host = {
  id: "password-host",
  label: "Password host",
  hostname: "password.example.test",
  port: 22,
  username: "deploy",
  auth: { password: { credential_key: "fixture-only" } },
  tags: [],
};
const agentHost: Host = {
  id: "atlas",
  label: "Atlas",
  hostname: "atlas.example.test",
  port: 22,
  username: "deploy",
  auth: "agent",
  tags: [],
};

let view: VueWrapper | undefined;
const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  terminals.items.length = 0;
  useVaultStore().unlocked = true;
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  setInvokeHandler("list_hosts", () => []);
  setInvokeHandler("list_groups", () => []);
  setInvokeHandler("list_identities", () => []);
});

afterEach(() => {
  view?.unmount();
  view = undefined;
  vi.unstubAllGlobals();
});

describe("persistent-view credential lifetime", () => {
  it("clears an unsubmitted SFTP password when Files is hidden without disconnecting live SFTP state", async () => {
    setInvokeHandler("list_hosts", () => [passwordHost]);
    view = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
    await flushPromises();

    await view.get("select").setValue(passwordHost.id);
    const input = view.get<HTMLInputElement>('input[type="password"]');
    await input.setValue("DO_NOT_RETAIN");
    expect(input.element.value).toBe("DO_NOT_RETAIN");

    const sftp = useSftpStore();
    sftp.sessionId = "existing-sftp-session";
    sftp.connectedHost = passwordHost;
    sftp.entries = [{ name: "README.md", long_name: "README.md", is_dir: false, is_file: true,
      is_symlink: false, size: 4, modified: null, permissions: null }];

    await view.setProps({ visible: false });
    expect(input.element.value).toBe("");
    expect(sftp.sessionId).toBe("existing-sftp-session");
    expect(sftp.entries.map(entry => entry.name)).toEqual(["README.md"]);
    expect(calls("sftp_close")).toHaveLength(0);

    await view.setProps({ visible: true });
    expect(view.get<HTMLInputElement>('input[type="password"]').element.value).toBe("");
    expect(sftp.sessionId).toBe("existing-sftp-session");
  });
});

describe("terminal listener recovery", () => {
  it("retries partial listener initialization and connects without duplicate data callbacks", async () => {
    useHostsStore().hosts = [agentHost];
    const tab = useTabsStore().newTab(agentHost);
    const pane = collectPanes(tab.tree)[0];

    const listenMock = vi.mocked(listen);
    const normalListen = listenMock.getMockImplementation();
    expect(normalListen).toBeDefined();
    listenMock.mockImplementationOnce(normalListen!);
    listenMock.mockRejectedValueOnce(new Error("fixture close-listener outage"));

    view = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });
    await vi.waitFor(() => expect(view!.get('[role="alert"]').text()).toContain("Could not initialize terminal"));
    expect(pane.connected).toBe(false);
    expect(calls("connect_ssh")).toHaveLength(0);
    expect(listenMock).toHaveBeenCalledTimes(2);

    await view.get('[role="alert"] button').trigger("click");
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(calls("connect_ssh")).toHaveLength(1);
    expect(listenMock).toHaveBeenCalledTimes(4);
    expect(view.find('[role="alert"]').exists()).toBe(false);

    emitTauriEvent("session-data", {
      session_id: pane.sessionId,
      data: Array.from(new TextEncoder().encode("ONLY_ONCE")),
    });
    await flushPromises();
    const rendered = terminals.items[0].output.join("");
    expect(rendered.match(/ONLY_ONCE/g)).toHaveLength(1);
  });
});
