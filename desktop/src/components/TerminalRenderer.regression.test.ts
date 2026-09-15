import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount, type VueWrapper } from "@vue/test-utils";
import { defineComponent, h, nextTick } from "vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { WEBGL_PANE_BUDGET, webglBackedPaneIds } from "../lib/terminalRenderer";
import type { Host } from "../types";

class FakeWebglAddon {
  static instances: FakeWebglAddon[] = [];
  disposed = 0;
  private handlers: (() => void)[] = [];
  constructor() { FakeWebglAddon.instances.push(this); }
  onContextLoss(handler: () => void) { this.handlers.push(handler); return { dispose() {} }; }
  loseContext() { this.handlers.forEach(handler => handler()); }
  dispose() { this.disposed += 1; }
}
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: FakeWebglAddon }));

vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80;
  rows = 24;
  addons: unknown[] = [];
  textarea = document.createElement("textarea");
  loadAddon(addon: unknown) { this.addons.push(addon); }
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div");
    root.className = "xterm";
    this.textarea.className = "xterm-helper-textarea";
    root.append(this.textarea); container.append(root);
  }
  onData() {}
  focus() {}
  write(_data: unknown, callback?: () => void) { callback?.(); }
  reset() {}
  dispose() { this.textarea.remove(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class { clearDecorations() {} onDidChangeResults() { return () => {}; } } }));

const host = (id: string): Host => ({ id, label: id,
  hostname: `${id}.example.test`, port: 22, username: "deploy", tags: [], auth: "agent" });

let view: VueWrapper | undefined;
function render(visible = true) {
  view = mount(defineComponent({ setup: () => () => h(TerminalWorkspace, { visible }) }), { attachTo: document.body });
  return view;
}
// The renderer is fetched through a dynamic import, so a claim needs a couple
// of microtask turns to settle before it shows up in the budget.
async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
    await nextTick();
  }
}

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  FakeWebglAddon.instances = [];
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(((kind: string) =>
    kind === "webgl2" ? { getExtension: () => null } : null) as never);
});
afterEach(() => {
  view?.unmount();
  view = undefined;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  expect(webglBackedPaneIds()).toEqual([]);
});

describe("terminal GPU renderer wiring", () => {
  it("puts a pane on the GPU renderer once it is on screen", async () => {
    const tabs = useTabsStore();
    const hosts = useHostsStore();
    hosts.hosts = [host("atlas")];
    const tab = tabs.newTab(hosts.hosts[0]);
    const pane = collectPanes(tab.tree)[0];
    render();
    await settle();
    expect(webglBackedPaneIds()).toEqual([pane.id]);
    expect(FakeWebglAddon.instances).toHaveLength(1);
  });

  it("leaves panes of a background tab on the DOM renderer until they are shown", async () => {
    const tabs = useTabsStore();
    const hosts = useHostsStore();
    hosts.hosts = [host("atlas"), host("orion")];
    const first = collectPanes(tabs.newTab(hosts.hosts[0]).tree)[0];
    const second = tabs.newTab(hosts.hosts[1]);
    const background = collectPanes(second.tree)[0];
    tabs.activeTabId = tabs.tabs[0].id;
    render();
    await settle();
    expect(webglBackedPaneIds()).toEqual([first.id]);
    tabs.activeTabId = second.id;
    await settle();
    expect(webglBackedPaneIds()).toEqual([first.id, background.id]);
  });

  it("does not claim the GPU renderer while the workspace itself is hidden", async () => {
    const tabs = useTabsStore();
    const hosts = useHostsStore();
    hosts.hosts = [host("atlas")];
    tabs.newTab(hosts.hosts[0]);
    render(false);
    await settle();
    expect(webglBackedPaneIds()).toEqual([]);
    expect(FakeWebglAddon.instances).toHaveLength(0);
  });

  it("hands the slot back when a pane closes", async () => {
    const tabs = useTabsStore();
    const hosts = useHostsStore();
    hosts.hosts = [host("atlas"), host("orion")];
    const tab = tabs.newTab(hosts.hosts[0]);
    const first = collectPanes(tab.tree)[0];
    const second = tabs.splitPane(first.id, "horizontal", hosts.hosts[1])!;
    render();
    await settle();
    expect(webglBackedPaneIds()).toHaveLength(2);
    tabs.closePane(second.id);
    await settle();
    expect(webglBackedPaneIds()).toEqual([first.id]);
    expect(FakeWebglAddon.instances.filter(addon => addon.disposed > 0)).toHaveLength(1);
  });

  it("keeps a heavy workspace inside the GPU context budget", async () => {
    const tabs = useTabsStore();
    const hosts = useHostsStore();
    hosts.hosts = [host("atlas")];
    const panes = [];
    for (let i = 0; i < WEBGL_PANE_BUDGET + 6; i++) {
      const tab = tabs.newTab(hosts.hosts[0]);
      panes.push({ tabId: tab.id, pane: collectPanes(tab.tree)[0] });
    }
    render();
    // Every pane of every tab stays mounted, so visit each tab in turn.
    for (const entry of panes) { tabs.activeTabId = entry.tabId; await settle(); }
    const backed = webglBackedPaneIds();
    expect(backed).toHaveLength(WEBGL_PANE_BUDGET);
    expect(backed[backed.length - 1]).toBe(panes[panes.length - 1].pane.id);
    // Every pane pushed out of the budget was reverted to the DOM renderer
    // rather than left holding a context, so exactly the budget stays live.
    const live = FakeWebglAddon.instances.filter(addon => addon.disposed === 0);
    expect(live).toHaveLength(WEBGL_PANE_BUDGET);
  });

  it("falls back to the DOM renderer when a pane loses its GPU context", async () => {
    const tabs = useTabsStore();
    const hosts = useHostsStore();
    hosts.hosts = [host("atlas")];
    const tab = tabs.newTab(hosts.hosts[0]);
    const pane = collectPanes(tab.tree)[0];
    render();
    await settle();
    expect(() => FakeWebglAddon.instances[0].loseContext()).not.toThrow();
    await settle();
    expect(webglBackedPaneIds()).toEqual([]);
    expect(FakeWebglAddon.instances[0].disposed).toBe(1);
    // The pane is still mounted and still the one the workspace is showing.
    expect(document.querySelector(`[data-pane-id="${pane.id}"] .xterm`)).not.toBeNull();
  });
});
