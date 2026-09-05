import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalPane from "./TerminalPane.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useUiStore } from "../stores/ui";
import { emitTauriEvent } from "../test/setup";
import type { Host } from "../types";

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    loadAddon() {}
    attachCustomKeyEventHandler() {}
    open() {}
    onData() { return { dispose() {} }; }
    focus() {}
    write() {}
    dispose() {}
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

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
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