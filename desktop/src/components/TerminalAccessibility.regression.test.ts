import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount, type VueWrapper } from "@vue/test-utils";
import { defineComponent, h, nextTick } from "vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import { useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useSettingsStore } from "../stores/settings";
import { webglBackedPaneIds } from "../lib/terminalRenderer";
import type { Host } from "../types";

class FakeWebglAddon {
  onContextLoss() { return { dispose() {} }; }
  dispose() {}
}
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: FakeWebglAddon }));

interface FakeOptions { screenReaderMode?: boolean }
interface FakeTerminal { options: FakeOptions }
const opened = vi.hoisted(() => ({ terminals: [] as FakeTerminal[] }));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    options: FakeOptions;
    textarea = document.createElement("textarea");
    constructor(options: FakeOptions) {
      this.options = { ...options };
      opened.terminals.push(this);
    }
    loadAddon() {}
    attachCustomKeyEventHandler() {}
    open(container: HTMLElement) {
      const root = document.createElement("div");
      root.className = "xterm";
      this.textarea.className = "xterm-helper-textarea";
      root.append(this.textarea);
      container.append(root);
    }
    onData() {}
    focus() {}
    write(_data: unknown, callback?: () => void) { callback?.(); }
    reset() {}
    dispose() { this.textarea.remove(); }
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({
  SearchAddon: class { clearDecorations() {} onDidChangeResults() { return () => {}; } },
}));

const host = (id: string): Host => ({
  id, label: id, hostname: `${id}.example.test`, port: 22,
  username: "deploy", tags: [], auth: "agent",
});

let view: VueWrapper | undefined;
async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
    await nextTick();
  }
}

// A terminal draws its rows for the eye: xterm marks the DOM renderer's rows
// `aria-hidden`, and the GPU renderer draws to a canvas, so neither is
// readable. What is readable is xterm's accessibility layer, which is built
// from the buffer and does not care which renderer is drawing — so the setting
// that turns it on must reach the terminal, and must not cost the GPU path.
describe("terminal screen reader support", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    localStorage.clear();
    // jsdom has no GPU; the budget only needs the probe to say yes.
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(((kind: string) =>
      kind === "webgl2" ? { getExtension: () => null } : null) as never);
    opened.terminals = [];
    webglBackedPaneIds().forEach(() => {});
    const hosts = useHostsStore();
    hosts.hosts = [host("host-1")];
  });

  afterEach(() => {
    view?.unmount();
    view = undefined;
    vi.restoreAllMocks();
  });

  async function open() {
    const tabs = useTabsStore();
    tabs.newTab(useHostsStore().hosts[0]);
    view = mount(defineComponent({ setup: () => () => h(TerminalWorkspace, { visible: true }) }),
      { attachTo: document.body });
    await settle();
  }

  it("is off by default, because it costs a DOM tree per terminal", async () => {
    await open();
    expect(opened.terminals).toHaveLength(1);
    expect(opened.terminals[0].options.screenReaderMode).toBe(false);
  });

  it("opens a terminal with the accessibility layer when it is enabled", async () => {
    useSettingsStore().setScreenReaderMode(true);
    await open();
    expect(opened.terminals[0].options.screenReaderMode).toBe(true);
  });

  it("does not give up the GPU renderer to stay accessible", async () => {
    // The accessibility layer reads the buffer, not the canvas, so the two are
    // not a trade: turning it on must not drop the pane back to DOM rendering.
    useSettingsStore().setScreenReaderMode(true);
    await open();
    expect(webglBackedPaneIds()).toHaveLength(1);
  });

  it("reaches terminals that are already open", async () => {
    await open();
    const terminal = opened.terminals[0];
    expect(terminal.options.screenReaderMode).toBe(false);

    useSettingsStore().setScreenReaderMode(true);
    await nextTick();

    expect(terminal.options.screenReaderMode).toBe(true);
  });

  it("is remembered across restarts", async () => {
    useSettingsStore().setScreenReaderMode(true);
    await nextTick();

    setActivePinia(createPinia());
    expect(useSettingsStore().screenReaderMode).toBe(true);
  });
});
