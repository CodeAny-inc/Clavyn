import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalArea from "./TerminalArea.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { useUiStore } from "../stores/ui";
import { emitSessionClosed, getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host, Identity } from "../types";

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    textarea = document.createElement("textarea");
    loadAddon() {}
    attachCustomKeyEventHandler() {}
    open(container: HTMLElement) {
      const root = document.createElement("div");
      root.className = "xterm";
      this.textarea.className = "xterm-helper-textarea";
      root.append(this.textarea);
      container.append(root);
    }
    onData() { return { dispose() {} }; }
    focus() { this.textarea.focus(); }
    write(_data: string | Uint8Array, callback?: () => void) { callback?.(); }
    reset() {}
    dispose() { this.textarea.remove(); }
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class { clearDecorations() {} onDidChangeResults() { return () => {}; } } }));

const host = (id: string, overrides: Partial<Host> = {}): Host => ({
  id, label: id, hostname: `${id}.example.test`, port: 22,
  username: "deploy", auth: "agent", tags: [], ...overrides,
});
const identity = (username: string): Identity => ({
  id: "shared", label: "Shared identity", username, auth: "agent", tags: [],
});
let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  // These existing scenarios inject already-loaded identity snapshots directly.
  // Delayed/failed initial loads are exercised in TerminalAuthentication.regression.
  useIdentitiesStore().loaded = true;
  document.body.innerHTML = "";
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  // jsdom does not implement native dialog focus restoration. Model that boundary
  // while keeping the real picker, Actions menu, pane focusin handler and store.
  const previousFocus = new WeakMap<HTMLDialogElement, Element | null>();
  vi.spyOn(HTMLDialogElement.prototype, "showModal").mockImplementation(function (this: HTMLDialogElement) {
    previousFocus.set(this, document.activeElement);
    this.open = true;
  });
  vi.spyOn(HTMLDialogElement.prototype, "close").mockImplementation(function (this: HTMLDialogElement) {
    this.open = false;
    const previous = previousFocus.get(this);
    if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
  });
});
afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function mountAtlas(overrides: Partial<Host> = {}) {
  const hosts = useHostsStore();
  hosts.hosts = [host("atlas", overrides), host("orion")];
  const tabs = useTabsStore();
  const tab = tabs.newTab(hosts.hosts[0]);
  const pane = collectPanes(tab.tree)[0];
  wrapper = mount(TerminalArea, { props: { visible: true }, attachTo: document.body });
  await vi.waitFor(() => expect(pane.connected).toBe(true));
  return { view: wrapper, tabs, pane, tab };
}
async function openFromActions(view: VueWrapper) {
  const trigger = view.get<HTMLButtonElement>('[aria-label="Actions for atlas"]');
  trigger.element.focus();
  await trigger.trigger("click");
  await vi.waitFor(() => expect(document.querySelector('[role="menu"]')).not.toBeNull());
  const action = Array.from(document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'))
    .find(button => button.textContent?.includes("Split right"))!;
  action.click();
  await nextTick();
  await vi.waitFor(() => expect(view.get<HTMLDialogElement>("dialog").element.open).toBe(true));
  return trigger;
}
function endpoint(view: VueWrapper) {
  return view.get('[data-host-id="atlas"] [data-testid="pane-header"]').text();
}
async function reconnect(view: VueWrapper) {
  await view.get('[data-host-id="atlas"]').findAll("button")
    .find(button => button.text() === "Reconnect")!.trigger("click");
}

describe("session picker activation through the real pane menu", () => {
  it.each(["horizontal", "vertical", "tab"])("keeps the %s destination active after native focus restoration", async placement => {
    const { view, tabs, pane, tab } = await mountAtlas();
    const originalSession = pane.sessionId;
    await openFromActions(view);
    await view.get('[aria-label="Open session in"]').setValue(placement);
    await view.get('[aria-label="Connect orion"]').trigger("click");
    await vi.waitFor(() => {
      expect(tabs.activePane?.hostId).toBe("orion");
      expect(tabs.activePane?.connected).toBe(true);
      expect(document.activeElement).toBe(view.get('[data-host-id="orion"] textarea').element);
    });
    expect(tabs.tabs).toHaveLength(placement === "tab" ? 2 : 1);
    if (placement === "tab") expect(tabs.activeTabId).not.toBe(tab.id);
    else expect(tabs.activeTabId).toBe(tab.id);
    expect(pane.sessionId).toBe(originalSession);
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "connect_ssh")).toHaveLength(2);
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "close_session")).toHaveLength(0);
  });

  it("preserves keyboard focus when an inactive pane's Actions control activates it", async () => {
    const { view, tabs, pane } = await mountAtlas();
    const orion = tabs.splitPane(pane.id, "horizontal", useHostsStore().hosts[1])!;
    await vi.waitFor(() => {
      expect(orion.connected).toBe(true);
      expect(document.activeElement).toBe(view.get('[data-host-id="orion"] textarea').element);
    });
    const trigger = view.get<HTMLButtonElement>('[aria-label="Actions for atlas"]');
    trigger.element.focus();
    await nextTick();
    await nextTick();
    expect(tabs.activePaneId).toBe(pane.id);
    expect(document.activeElement).toBe(trigger.element);
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    await vi.waitFor(() => {
      const firstAction = document.querySelector('[role="menuitem"]');
      expect(firstAction).not.toBeNull();
      expect(document.activeElement).toBe(firstAction);
    });
  });

  it("returns cancellation focus to the source without creating or closing sessions", async () => {
    const { view, tabs, pane } = await mountAtlas();
    const trigger = await openFromActions(view);
    await view.get('[aria-label="Close session picker"]').trigger("click");
    expect(document.activeElement).toBe(trigger.element);
    expect(tabs.activePaneId).toBe(pane.id);
    expect(tabs.tabs).toHaveLength(1);
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "connect_ssh")).toHaveLength(1);
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "close_session")).toHaveLength(0);
  });
});

describe("effective SSH identity snapshots", () => {
  it("uses the identity username and only refreshes the live snapshot on successful reconnect", async () => {
    const identities = useIdentitiesStore();
    identities.identities = [identity("root")];
    const { view, pane } = await mountAtlas({ identity_id: "shared" });
    expect(endpoint(view)).toContain("root@atlas.example.test:22");
    const original = pane.sessionId;
    identities.identities = [identity("ops")];
    await nextTick();
    expect(endpoint(view)).toContain("root@atlas.example.test:22");
    emitSessionClosed(original, "test disconnect");
    await nextTick();
    setInvokeHandler("connect_ssh", () => { throw new Error("connection refused"); });
    await reconnect(view);
    await vi.waitFor(() => expect(view.text()).toContain("connection refused"));
    expect(endpoint(view)).toContain("root@atlas.example.test:22");
    setInvokeHandler("connect_ssh", () => undefined);
    await reconnect(view);
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(pane.sessionId).not.toBe(original);
    expect(endpoint(view)).toContain("ops@atlas.example.test:22");
    identities.identities = [];
    await nextTick();
    expect(endpoint(view)).toContain("ops@atlas.example.test:22");
  });

  it("falls back to the host username for a direct host without an identity link", async () => {
    const { view } = await mountAtlas({ identity_id: undefined });
    expect(endpoint(view)).toContain("deploy@atlas.example.test:22");
  });

  it("rejects a broken identity reference instead of silently using stale host credentials", async () => {
    const hosts = useHostsStore();
    hosts.hosts = [host("atlas", { identity_id: "missing" }), host("orion")];
    const tabs = useTabsStore();
    const tab = tabs.newTab(hosts.hosts[0]);
    const pane = collectPanes(tab.tree)[0];
    wrapper = mount(TerminalArea, { props: { visible: true }, attachTo: document.body });
    await vi.waitFor(() => expect(wrapper!.text()).toContain("Linked SSH identity not found"));
    expect(pane.connected).toBe(false);
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "connect_ssh")).toHaveLength(0);
  });

  it("captures the identity at dispatch rather than when the connection resolves", async () => {
    const identities = useIdentitiesStore();
    identities.identities = [identity("root")];
    let complete: (() => void) | undefined;
    setInvokeHandler("connect_ssh", () => new Promise<void>(resolve => { complete = resolve; }));
    const mounted = mountAtlas({ identity_id: "shared" });
    await vi.waitFor(() => expect(complete).toBeDefined());
    identities.identities = [identity("ops")];
    complete!();
    const { view } = await mounted;
    expect(endpoint(view)).toContain("root@atlas.example.test:22");
  });

  it("resolves the username after waiting for a vault unlock", async () => {
    const identities = useIdentitiesStore();
    identities.identities = [{ ...identity("root"), auth: "publickey" }];
    let unlock: ((success: boolean) => void) | undefined;
    vi.spyOn(useUiStore(), "requestVaultUnlock").mockImplementation(() => new Promise<boolean>(resolve => { unlock = resolve; }));
    const mounted = mountAtlas({ identity_id: "shared" });
    await vi.waitFor(() => expect(unlock).toBeDefined());
    identities.identities = [{ ...identity("ops"), auth: "publickey" }];
    unlock!(true);
    const { view } = await mounted;
    expect(endpoint(view)).toContain("ops@atlas.example.test:22");
  });
});
