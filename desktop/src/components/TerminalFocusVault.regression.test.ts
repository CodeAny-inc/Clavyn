import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { DOMWrapper, mount, type VueWrapper } from "@vue/test-utils";
import { defineComponent, h, nextTick } from "vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import CommandPalette from "./CommandPalette.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useUiStore } from "../stores/ui";
import { useVaultStore } from "../stores/vault";
import { emitTauriEvent, getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host } from "../types";

vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80;
  rows = 24;
  textarea = document.createElement("textarea");
  input: (data: string) => void = () => {};
  loadAddon() {}
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div");
    root.className = "xterm";
    this.textarea.className = "xterm-helper-textarea";
    this.textarea.addEventListener("keydown", event => {
      if (event.key.length === 1 || event.key === "Enter") this.input(event.key === "Enter" ? "\r" : event.key);
    });
    root.append(this.textarea); container.append(root);
  }
  onData(handler: (data: string) => void) { this.input = handler; }
  focus() { this.textarea.focus(); }
  write(_data: unknown, callback?: () => void) { callback?.(); }
  reset() {}
  dispose() { this.textarea.remove(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class { clearDecorations() {} } }));

const host = (id: string, auth: Host["auth"] = "agent"): Host => ({ id, label: id,
  hostname: `${id}.example.test`, port: 22, username: "deploy", tags: [], auth });
let view: VueWrapper | undefined;
const calls = (command: string) => getInvokeMock().mock.calls.filter(([cmd]) => cmd === command);
function render() {
  view = mount(defineComponent({ setup() {
    const ui = useUiStore();
    return () => h("div", [h(TerminalWorkspace, { visible: true }),
      h(CommandPalette, { open: ui.commandPaletteOpen, onClose: () => { ui.commandPaletteOpen = false; } })]);
  } }), { attachTo: document.body });
  return view;
}
function delay(command = "connect_ssh") {
  let resolve!: () => void;
  setInvokeHandler(command, () => new Promise<void>(done => { resolve = done; }));
  return () => resolve();
}
async function palette() {
  useUiStore().commandPaletteOpen = true;
  await nextTick(); await nextTick();
  const input = new DOMWrapper(document.querySelector<HTMLInputElement>('input[placeholder="Search commands..."]')!);
  await input.setValue("Go to ");
  expect(document.activeElement).toBe(input.element);
  return input;
}
async function pair(auth: Host["auth"] = "publickey") {
  const tabs = useTabsStore();
  const hosts = useHostsStore();
  hosts.hosts = [host("atlas", auth), host("orion", auth)];
  const tab = tabs.newTab(hosts.hosts[0]);
  const first = collectPanes(tab.tree)[0];
  const second = tabs.splitPane(first.id, "horizontal", hosts.hosts[1])!;
  const request = vi.spyOn(useUiStore(), "requestVaultUnlock");
  const wrapper = render();
  return { tabs, first, second, wrapper, request };
}
beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => { view?.unmount(); view = undefined; vi.unstubAllGlobals(); });

describe("terminal focus ownership", () => {
  it.each(["initial SSH", "reconnect SSH", "initial local"])("does not route palette input to a shell after %s completes", async mode => {
    const saved = host("atlas");
    useHostsStore().hosts = [saved];
    const tab = useTabsStore().newTab(mode === "initial local" ? undefined : saved);
    const pane = collectPanes(tab.tree)[0];
    let release: () => void;
    if (mode === "reconnect SSH") {
      render();
      await vi.waitFor(() => expect(pane.connected).toBe(true));
      emitTauriEvent("session-closed", { session_id: pane.sessionId, reason: "fixture EOF" });
      release = delay();
      await nextTick();
      await view!.get('[role="alert"] button').trigger("click");
      await vi.waitFor(() => expect(calls("connect_ssh")).toHaveLength(2));
    } else {
      const command = mode === "initial local" ? "create_local_terminal" : "connect_ssh";
      release = delay(command);
      render();
      await vi.waitFor(() => expect(calls(command)).toHaveLength(1));
    }
    const input = await palette();
    release();
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(document.activeElement).toBe(input.element);
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "H", bubbles: true }));
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(calls("session_write")).toHaveLength(0);
  });

  it("keeps Actions keyboard focus when a pending connection completes", async () => {
    const release = delay();
    const saved = host("atlas");
    useHostsStore().hosts = [saved];
    const pane = collectPanes(useTabsStore().newTab(saved).tree)[0];
    render();
    await vi.waitFor(() => expect(calls("connect_ssh")).toHaveLength(1));
    await view!.get('[aria-label="Actions for atlas"]').trigger("click");
    await nextTick();
    const focused = document.activeElement;
    expect(focused?.getAttribute("role")).toBe("menuitem");
    release();
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(document.activeElement).toBe(focused);
    expect(calls("session_write")).toHaveLength(0);
  });

  it("does not autofocus a delayed password prompt over the palette", async () => {
    let resolve!: (items: unknown[]) => void;
    setInvokeHandler("list_identities", () => new Promise(done => { resolve = done; }));
    const saved = host("atlas", { password: { credential_key: "fixture-only" } });
    useHostsStore().hosts = [saved];
    useTabsStore().newTab(saved);
    render();
    await vi.waitFor(() => expect(calls("list_identities")).toHaveLength(1));
    const input = await palette();
    resolve([]);
    await vi.waitFor(() => expect(view!.find('input[aria-label="SSH password"]').exists()).toBe(true));
    expect(document.activeElement).toBe(input.element);
    expect(calls("session_write")).toHaveLength(0);
    expect(calls("connect_ssh")).toHaveLength(0);
  });

  it("does not let a queued structural move steal overlay focus", async () => {
    const { tabs, first, second } = await pair("agent");
    await vi.waitFor(() => expect(second.connected && first.connected).toBe(true));
    const input = await palette();
    tabs.startDrag(second.id); tabs.dropPane(first.id, "left");
    await nextTick(); await nextTick();
    expect(document.activeElement).toBe(input.element);
    expect(calls("close_session")).toHaveLength(0);
  });

  it("restores the password owner's input at submit, without waiting for the network", async () => {
    const release = delay();
    const saved = host("atlas", { password: { credential_key: "fixture-only" } });
    useHostsStore().hosts = [saved];
    const pane = collectPanes(useTabsStore().newTab(saved).tree)[0];
    render();
    await vi.waitFor(() => expect(view!.find('input[aria-label="SSH password"]').exists()).toBe(true));
    await view!.get('input[aria-label="SSH password"]').setValue("fixture-only password");
    await view!.get('form').trigger("submit");
    await vi.waitFor(() => expect(calls("connect_ssh")).toHaveLength(1));
    expect(document.activeElement).toBe(view!.get("textarea").element);
    const input = await palette();
    release();
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(document.activeElement).toBe(input.element);
    expect(calls("session_write")).toHaveLength(0);
  });
});

describe("concurrent vault-dependent panes", () => {
  it("connects both panes exactly once after one successful unlock", async () => {
    const { first, second, request } = await pair();
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    expect(calls("connect_ssh")).toHaveLength(0);
    useVaultStore().unlocked = true;
    useUiStore().resolveVaultUnlock(true);
    await vi.waitFor(() => expect(first.connected && second.connected).toBe(true));
    expect(calls("connect_ssh")).toHaveLength(2);
    expect(new Set(calls("connect_ssh").map(([, args]) => args.sessionId)).size).toBe(2);
  });

  it("settles cancellation for both panes and permits a fresh retry cycle", async () => {
    const { first, second, request, wrapper } = await pair();
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    useUiStore().resolveVaultUnlock(false);
    await vi.waitFor(() => expect(wrapper.findAll('[role="alert"]')).toHaveLength(2));
    expect(calls("connect_ssh")).toHaveLength(0);
    const buttons = wrapper.findAll('[role="alert"] button');
    expect(buttons.every(button => !(button.element as HTMLButtonElement).disabled)).toBe(true);
    for (const button of buttons) await button.trigger("click");
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(4));
    useVaultStore().unlocked = true;
    useUiStore().resolveVaultUnlock(true);
    await vi.waitFor(() => expect(first.connected && second.connected).toBe(true));
    expect(calls("connect_ssh")).toHaveLength(2);
  });

  it("does not authenticate a closed waiter when the shared unlock succeeds", async () => {
    const { tabs, first, second, request } = await pair();
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    tabs.closePane(first.id);
    await nextTick();
    useVaultStore().unlocked = true;
    useUiStore().resolveVaultUnlock(true);
    await vi.waitFor(() => expect(second.connected).toBe(true));
    expect(calls("connect_ssh")).toHaveLength(1);
    expect(calls("connect_ssh")[0][1].host.id).toBe("orion");
  });

  it("shares unlock when both backends reject an apparently unlocked vault", async () => {
    const attempted = new Set<string>();
    useVaultStore().unlocked = true;
    setInvokeHandler("connect_ssh", (args: { sessionId: string }) => {
      if (!attempted.has(args.sessionId)) { attempted.add(args.sessionId); throw new Error("vault required"); }
    });
    const { first, second, request } = await pair();
    await vi.waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    useUiStore().resolveVaultUnlock(true);
    await vi.waitFor(() => expect(first.connected && second.connected).toBe(true));
    expect(calls("connect_ssh")).toHaveLength(4);
  });
});
