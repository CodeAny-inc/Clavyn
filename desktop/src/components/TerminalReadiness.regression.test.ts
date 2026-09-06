import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { emitTauriEvent, getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host } from "../types";

const terminals = vi.hoisted(() => ({ instances: [] as Array<{
  textarea: HTMLTextAreaElement; output: string[]; input: (data: string) => void;
}> }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80;
  rows = 24;
  textarea = document.createElement("textarea");
  output: string[] = [];
  input: (data: string) => void = () => {};
  constructor() { terminals.instances.push(this); }
  loadAddon() {}
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div");
    root.className = "xterm";
    this.textarea.className = "xterm-helper-textarea";
    root.append(this.textarea); container.append(root);
  }
  onData(handler: (data: string) => void) { this.input = handler; }
  focus() { this.textarea.focus(); }
  write(data: string | Uint8Array, callback?: () => void) {
    const text = typeof data === "string" ? data : new TextDecoder().decode(data);
    this.output.push(text);
    // Synchronous reply catches a flush performed before the write path is ready.
    // The browser suite additionally exercises the real asynchronous xterm parser.
    if (text.includes("\x1b[6n")) this.input("\x1b[3;3R");
    callback?.();
  }
  reset() { this.output.length = 0; }
  dispose() { this.textarea.remove(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class { clearDecorations() {} } }));

const host = (id: string, password = false): Host => ({ id, label: id,
  hostname: `${id}.example.test`, port: 22, username: "deploy", tags: [],
  auth: password ? { password: { credential_key: "fixture-only" } } : "agent" });
let view: VueWrapper | undefined;
function render() {
  view = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });
  return view;
}
function output(id: string, text: string) {
  emitTauriEvent("session-data", { session_id: id, data: Array.from(new TextEncoder().encode(text)) });
}
function writes() { return getInvokeMock().mock.calls.filter(([cmd]) => cmd === "session_write"); }
function deferConnection(command: string) {
  let id = "";
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  setInvokeHandler(command, (args: { sessionId: string }) => {
    id = args.sessionId;
    return new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  });
  return { get id() { return id; }, resolve: () => resolve(), reject: () => reject(new Error("fixture failure")) };
}
async function split(password = false) {
  const tabs = useTabsStore();
  const hosts = useHostsStore();
  hosts.hosts = [host("atlas"), host("orion", password)];
  const tab = tabs.newTab(hosts.hosts[0]);
  const first = collectPanes(tab.tree)[0];
  const second = tabs.splitPane(first.id, "horizontal", hosts.hosts[1])!;
  const wrapper = render();
  await vi.waitFor(() => expect(first.connected).toBe(true));
  if (password) await vi.waitFor(() => expect(wrapper.find('input[aria-label="SSH password"]').exists()).toBe(true));
  else await vi.waitFor(() => expect(second.connected).toBe(true));
  return { tabs, first, second, wrapper };
}
beforeEach(() => {
  setActivePinia(createPinia());
  terminals.instances.length = 0;
  document.body.innerHTML = "";
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => { view?.unmount(); view = undefined; vi.unstubAllGlobals(); });

describe("connection output readiness", () => {
  it.each(["connect_ssh", "create_local_terminal"])("flushes early output only after %s can accept protocol replies", async command => {
    const pending = deferConnection(command);
    const saved = host("atlas");
    useHostsStore().hosts = [saved];
    const tab = useTabsStore().newTab(command === "connect_ssh" ? saved : undefined);
    const pane = collectPanes(tab.tree)[0];
    render();
    await vi.waitFor(() => expect(pending.id).not.toBe(""));
    output(pending.id, "\x1b[6n");
    terminals.instances[0].input("DO_NOT_REPLAY_USER_INPUT");
    expect(writes()).toHaveLength(0);
    expect(terminals.instances[0].output).not.toContain("\x1b[6n");
    pending.resolve();
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(writes()).toHaveLength(1);
    expect(writes()[0][1]).toEqual({ sessionId: pending.id, data: Array.from(new TextEncoder().encode("\x1b[3;3R")) });
  });

  it.each(["failure", "closed", "disposed"])("discards pending output on %s", async ending => {
    const pending = deferConnection("connect_ssh");
    const saved = host("atlas");
    useHostsStore().hosts = [saved];
    const tab = useTabsStore().newTab(saved);
    const pane = collectPanes(tab.tree)[0];
    render();
    await vi.waitFor(() => expect(pending.id).not.toBe(""));
    output(pending.id, "\x1b[6n");
    if (ending === "failure") pending.reject();
    else {
      if (ending === "closed") emitTauriEvent("session-closed", { session_id: pending.id, reason: "fixture EOF" });
      else { view!.unmount(); view = undefined; }
      pending.resolve();
    }
    await new Promise(resolve => setTimeout(resolve, 0));
    output(pending.id, "\x1b[6n");
    expect(pane.connected).toBe(false);
    expect(writes()).toHaveLength(0);
    expect(terminals.instances[0].output).not.toContain("\x1b[6n");
  });

  it("does not carry a failed attempt's output into its replacement", async () => {
    const pending = deferConnection("connect_ssh");
    const saved = host("atlas");
    useHostsStore().hosts = [saved];
    useTabsStore().newTab(saved);
    const wrapper = render();
    await vi.waitFor(() => expect(pending.id).not.toBe(""));
    const oldId = pending.id;
    output(oldId, "STALE\x1b[6n");
    pending.reject();
    await vi.waitFor(() => expect(wrapper.text()).toContain("fixture failure"));
    const replacement = deferConnection("connect_ssh");
    await wrapper.get('[role="alert"] button').trigger("click");
    await vi.waitFor(() => expect(replacement.id).not.toBe(""));
    output(oldId, "STALE\x1b[6n");
    output(replacement.id, "\x1b[6n");
    replacement.resolve();
    await vi.waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0][1].sessionId).toBe(replacement.id);
    expect(terminals.instances[0].output.join("")).not.toContain("STALE");
  });
});

describe("post-move focus requests", () => {
  it.each(["left", "center", "bottom"] as const)("restores input after an already-active pane moves to %s", async position => {
    const { tabs, first, second, wrapper } = await split();
    const textarea = wrapper.get('[data-host-id="orion"] textarea').element as HTMLTextAreaElement;
    textarea.focus(); textarea.blur(); // Model native drag focus loss; never repair it in the assertion.
    tabs.startDrag(second.id); tabs.dropPane(first.id, position);
    await vi.waitFor(() => expect(document.activeElement).toBe(textarea));
    expect(tabs.activePaneId).toBe(second.id);
    expect(terminals.instances).toHaveLength(2);
    expect(getInvokeMock().mock.calls.filter(([cmd]) => cmd === "close_session")).toHaveLength(0);
  });
  it.each([false, true])("prefers the moved pane's search/password field (password=%s)", async password => {
    const { tabs, first, second, wrapper } = await split(password);
    if (!password) await wrapper.get('[data-host-id="orion"] [aria-label="Search in terminal"]').trigger("click");
    const input = wrapper.get(`[data-host-id="orion"] input[aria-label="${password ? "SSH password" : "Search terminal output"}"]`).element as HTMLInputElement;
    input.focus(); input.blur();
    tabs.startDrag(second.id); tabs.dropPane(first.id, "left");
    await vi.waitFor(() => expect(document.activeElement).toBe(input));
    expect(writes()).toHaveLength(0);
  });
  it("does not restore an old destination after a newer selection", async () => {
    const { tabs, first, second, wrapper } = await split();
    tabs.startDrag(second.id); tabs.dropPane(first.id, "left");
    tabs.setActivePane(first.id);
    await nextTick(); await nextTick();
    expect(document.activeElement).toBe(wrapper.get('[data-host-id="atlas"] textarea').element);
  });
  it("does not request focus for a rejected self-drop", async () => {
    const { tabs, second, wrapper } = await split();
    const control = wrapper.get('[data-host-id="orion"] [aria-label="Actions for orion"]').element as HTMLButtonElement;
    control.focus();
    tabs.startDrag(second.id); tabs.dropPane(second.id, "center");
    await nextTick(); await nextTick();
    expect(document.activeElement).toBe(control);
  });
});
