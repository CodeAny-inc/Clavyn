import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalPane from "./TerminalPane.vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useUiStore } from "../stores/ui";
import { emitTauriEvent, setInvokeHandler } from "../test/setup";
import type { Host } from "../types";

const terminalMock = vi.hoisted(() => ({
  reset: vi.fn(),
  write: vi.fn((_data: string | Uint8Array, callback?: () => void) => callback?.()),
}));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    textarea = document.createElement("textarea");
    keyHandler?: (event: KeyboardEvent) => boolean;
    loadAddon() {}
    attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean) { this.keyHandler = handler; }
    open(container: HTMLElement) {
      const root = document.createElement("div");
      root.className = "xterm";
      this.textarea.className = "xterm-helper-textarea";
      this.textarea.addEventListener("keydown", event => this.keyHandler?.(event));
      root.append(this.textarea);
      container.append(root);
    }
    onData() { return { dispose() {} }; }
    focus() { this.textarea.focus(); }
    write(data: string | Uint8Array, callback?: () => void) { terminalMock.write(data, callback); }
    reset() { terminalMock.reset(); }
    dispose() { this.textarea.remove(); }
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({
  SearchAddon: class {
    clearDecorations() {}
    findNext() { return true; }
    findPrevious() { return true; }
  },
}));

const ActionMenuStub = {
  props: ["label"],
  template: '<button type="button" :aria-label="label">Actions</button>',
};
const makeHost = (id: string, overrides: Partial<Host> = {}): Host => ({
  id,
  label: id === "atlas" ? "Atlas" : "Orion",
  hostname: `${id}.example.test`,
  port: 22,
  username: "deploy",
  auth: "agent",
  tags: [],
  ...overrides,
});

let wrapper: VueWrapper | undefined;
function mountPane(pane: ReturnType<typeof collectPanes>[number], tabId: string) {
  wrapper = mount(TerminalPane, {
    props: { pane, tabId, visible: true },
    attachTo: document.body,
    global: { stubs: { ActionMenu: ActionMenuStub } },
  });
  return wrapper;
}
async function mountSplit() {
  const hosts = useHostsStore();
  const tabs = useTabsStore();
  const atlas = makeHost("atlas");
  const orion = makeHost("orion");
  hosts.hosts = [atlas, orion];
  const tab = tabs.newTab(atlas);
  const first = collectPanes(tab.tree)[0];
  const second = tabs.splitPane(first.id, "horizontal", orion)!;
  wrapper = mount(TerminalWorkspace, {
    props: { visible: true },
    attachTo: document.body,
    global: { stubs: { ActionMenu: ActionMenuStub } },
  });
  await vi.waitFor(() => {
    expect(first.connected).toBe(true);
    expect(second.connected).toBe(true);
  });
  return { view: wrapper, tabs, tab, first, second };
}

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  terminalMock.reset.mockClear();
  terminalMock.write.mockReset();
  terminalMock.write.mockImplementation((_data, callback) => callback?.());
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    disconnect() {}
  });
});
afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  vi.unstubAllGlobals();
});

describe("TerminalPane session identity", () => {
  it("keeps the displayed endpoint tied to the successful session until reconnect succeeds", async () => {
    const hosts = useHostsStore();
    const tabs = useTabsStore();
    const original = makeHost("atlas");
    hosts.hosts = [original];
    const tab = tabs.newTab(original);
    const pane = collectPanes(tab.tree)[0];
    const view = mountPane(pane, tab.id);

    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(view.get('[data-testid="pane-header"]').text()).toContain("deploy@atlas.example.test:22");
    const firstSession = pane.sessionId!;

    hosts.hosts = [{ ...original, username: "ops", hostname: "new.example.test", port: 2200 }];
    await nextTick();
    expect(view.get('[data-testid="pane-header"]').text()).toContain("deploy@atlas.example.test:22");
    expect(view.get('[data-testid="pane-header"]').text()).not.toContain("ops@new.example.test:2200");

    emitTauriEvent("session-closed", { session_id: firstSession, reason: "test disconnect" });
    await vi.waitFor(() => expect(pane.connected).toBe(false));
    const reconnect = view.findAll("button").find(button => button.text() === "Reconnect");
    expect(reconnect).toBeDefined();
    await reconnect!.trigger("click");
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(pane.sessionId).not.toBe(firstSession);
    expect(view.get('[data-testid="pane-header"]').text()).toContain("ops@new.example.test:2200");

    hosts.hosts = [];
    await nextTick();
    expect(view.get('[data-testid="pane-header"]').text()).toContain("ops@new.example.test:2200");
    expect(view.get('[data-testid="pane-header"]').text()).not.toContain("Local shell");
  });

  it("activates an inactive pane before search, fullscreen and action-menu controls", async () => {
    const hosts = useHostsStore();
    const tabs = useTabsStore();
    const ui = useUiStore();
    const atlas = makeHost("atlas");
    const orion = makeHost("orion");
    hosts.hosts = [atlas, orion];
    const tab = tabs.newTab(atlas);
    const first = collectPanes(tab.tree)[0];
    const second = tabs.splitPane(first.id, "horizontal", orion)!;
    expect(tabs.activePaneId).toBe(second.id);
    const view = mountPane(first, tab.id);
    await vi.waitFor(() => expect(first.connected).toBe(true));
    expect(tabs.activePaneId).toBe(second.id);

    await view.get('button[aria-label="Search in terminal"]').trigger("click");
    expect(tabs.activePaneId).toBe(first.id);
    expect(view.find('input[aria-label="Search terminal output"]').exists()).toBe(true);
    await view.get('button[aria-label="Close search"]').trigger("click");
    expect(tabs.activePaneId).toBe(first.id);

    tabs.setActivePane(second.id);
    await nextTick();
    await view.get('button[aria-label="Fullscreen"]').trigger("click");
    expect(tabs.activePaneId).toBe(first.id);
    expect(ui.fullscreenPaneId).toBe(first.id);

    ui.exitFullscreen();
    tabs.setActivePane(second.id);
    await nextTick();
    await view.get('button[aria-label="Actions for Atlas"]').trigger("pointerdown");
    expect(tabs.activePaneId).toBe(first.id);
  });
});

describe("TerminalPane input ownership", () => {
  it.each([false, true])("exits fullscreen before a pane-navigation shortcut transfers focus (meta=%s)", async metaKey => {
    const { view, tabs, first, second } = await mountSplit();
    const ui = useUiStore();
    await view.get('[data-host-id="atlas"] button[aria-label="Fullscreen"]').trigger("click");
    expect(ui.fullscreenPaneId).toBe(first.id);
    await view.get('[data-host-id="atlas"] textarea').trigger("keydown", {
      key: "ArrowRight", ctrlKey: !metaKey, metaKey,
    });
    expect(tabs.activePaneId).toBe(second.id);
    expect(ui.fullscreenPaneId).toBeNull();
    await vi.waitFor(() => expect(document.activeElement).toBe(view.get('[data-host-id="orion"] textarea').element));
  });

  it("clears stale fullscreen synchronously for store-driven activation too", async () => {
    const { view, tabs, second } = await mountSplit();
    await view.get('[data-host-id="atlas"] button[aria-label="Fullscreen"]').trigger("click");
    tabs.setActivePane(second.id);
    // Do not await nextTick: the old fullscreen must already be invalidated.
    expect(useUiStore().fullscreenPaneId).toBeNull();
  });

  it("restores the destination search input and returns Escape to that pane's terminal", async () => {
    const { view, tabs, first } = await mountSplit();
    await view.get('[data-host-id="atlas"] button[aria-label="Search in terminal"]').trigger("click");
    const search = view.get('[data-host-id="atlas"] input[aria-label="Search terminal output"]');
    expect(document.activeElement).toBe(search.element);
    await view.get('[data-host-id="orion"]').trigger("click");
    await vi.waitFor(() => expect(document.activeElement).toBe(view.get('[data-host-id="orion"] textarea').element));
    await view.get('[data-host-id="orion"] textarea').trigger("keydown", { key: "ArrowLeft", ctrlKey: true });
    expect(tabs.activePaneId).toBe(first.id);
    await vi.waitFor(() => expect(document.activeElement).toBe(search.element));
    await search.trigger("keydown", { key: "Escape" });
    expect(document.activeElement).toBe(view.get('[data-host-id="atlas"] textarea').element);
  });

  it("does not reset on layout changes, and drains old output before reset and replacement connect", async () => {
    const { view, tabs, tab, first, second } = await mountSplit();
    const old = first.sessionId;
    tabs.startDrag(first.id);
    tabs.dropPane(second.id, "center");
    const other = tabs.newTab();
    await nextTick();
    tabs.setActiveTab(tab.id);
    tabs.closeTab(other.id);
    await nextTick();
    expect(terminalMock.reset).not.toHaveBeenCalled();
    let drained: (() => void) | undefined;
    terminalMock.write.mockImplementation((data, callback) => {
      if (data === "" && callback) drained = callback;
      else callback?.();
    });
    const connect = vi.fn(() => { expect(terminalMock.reset).toHaveBeenCalledTimes(1); });
    setInvokeHandler("connect_ssh", connect);
    emitTauriEvent("session-closed", { session_id: old, reason: "test disconnect" });
    await nextTick();
    const reconnect = view.get('[data-host-id="atlas"]').findAll("button").find(button => button.text() === "Reconnect")!;
    await reconnect.trigger("click");
    await vi.waitFor(() => expect(drained).toBeDefined());
    expect(connect).not.toHaveBeenCalled();
    expect(terminalMock.reset).not.toHaveBeenCalled();
    drained!();
    await vi.waitFor(() => expect(first.connected).toBe(true));
    expect(terminalMock.reset).toHaveBeenCalledTimes(1);
    expect(connect).toHaveBeenCalledTimes(1);
    expect(first.sessionId).not.toBe(old);
    expect(second.connected).toBe(true);
  });

  it("does not start a replacement if the pane is disposed while draining output", async () => {
    const { view, first } = await mountSplit();
    let drained: (() => void) | undefined;
    terminalMock.write.mockImplementation((data, callback) => {
      if (data === "" && callback) drained = callback;
      else callback?.();
    });
    const connect = vi.fn();
    setInvokeHandler("connect_ssh", connect);
    emitTauriEvent("session-closed", { session_id: first.sessionId, reason: "test disconnect" });
    await nextTick();
    await view.get('[data-host-id="atlas"]').findAll("button").find(button => button.text() === "Reconnect")!.trigger("click");
    await vi.waitFor(() => expect(drained).toBeDefined());
    view.unmount();
    wrapper = undefined;
    drained!();
    await nextTick();
    expect(connect).not.toHaveBeenCalled();
    expect(terminalMock.reset).not.toHaveBeenCalled();
  });
});
