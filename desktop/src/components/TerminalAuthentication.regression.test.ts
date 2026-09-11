import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import SessionPicker from "./SessionPicker.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { emitSessionClosed, getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host, Identity } from "../types";

vi.mock("@xterm/xterm", () => ({ Terminal: class {
  cols = 80; rows = 24;
  textarea = document.createElement("textarea");
  loadAddon() {}
  attachCustomKeyEventHandler() {}
  open(container: HTMLElement) {
    const root = document.createElement("div"); root.className = "xterm";
    this.textarea.className = "xterm-helper-textarea"; root.append(this.textarea); container.append(root);
  }
  onData() { return { dispose() {} }; }
  focus() { this.textarea.focus(); }
  write(_data: string | Uint8Array, callback?: () => void) { callback?.(); }
  reset() {}
  dispose() { this.textarea.remove(); }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class { clearDecorations() {} onDidChangeResults() { return () => {}; } } }));
const passwordMethod = { password: { credential_key: "fixture-metadata" } };
const host = (overrides: Partial<Host> = {}): Host => ({
  id: "atlas", label: "Atlas", hostname: "atlas.example.test", port: 22,
  username: "deploy", auth: "agent", tags: [], ...overrides,
});
const identity = (overrides: Partial<Identity> = {}): Identity => ({
  id: "shared", label: "Shared", username: "root", auth: "agent", tags: [], ...overrides,
});
let view: VueWrapper | undefined;
const calls = () => getInvokeMock().mock.calls.filter(([name]) => name === "connect_ssh");
function begin(overrides: Partial<Host> = {}) {
  const saved = host(overrides);
  useHostsStore().hosts = [saved];
  const tabs = useTabsStore();
  const tab = tabs.newTab(saved);
  const pane = collectPanes(tab.tree)[0];
  view = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });
  return { tabs, pane, tab };
}
async function passwordInput() {
  await vi.waitFor(() => expect(view!.find('input[aria-label="SSH password"]').exists()).toBe(true));
  return view!.get<HTMLInputElement>('input[aria-label="SSH password"]');
}
async function submit(value: string) {
  const input = await passwordInput();
  await input.setValue(value);
  await view!.get("form").trigger("submit");
  return input.element;
}
beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.spyOn(HTMLDialogElement.prototype, "showModal").mockImplementation(function (this: HTMLDialogElement) { this.open = true; });
  vi.spyOn(HTMLDialogElement.prototype, "close").mockImplementation(function (this: HTMLDialogElement) { this.open = false; });
});
afterEach(() => {
  view?.unmount(); view = undefined;
  vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("SSH identity load and dispatch boundaries", () => {
  it("waits for the real identity before dispatching an early HostList/workspace connection", async () => {
    let release!: (items: Identity[]) => void;
    setInvokeHandler("list_identities", () => new Promise<Identity[]>(resolve => { release = resolve; }));
    const { pane } = begin({ identity_id: "shared" });
    await vi.waitFor(() => expect(release).toBeDefined());
    expect(calls()).toHaveLength(0);
    expect(view!.get('[data-testid="pane-header"]').text()).not.toContain("deploy@");
    release([identity()]);
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    const args = calls()[0][1];
    expect(args.expectedUsername).toBe("root");
    expect(args.host.identity_id).toBeNull();
    expect(args.host.username).toBe("root");
    expect(args.host.auth).toBe("agent");
    expect(view!.text()).toContain("root@atlas.example.test:22");
  });
  it("blocks a failed identity load and retries it on reconnect", async () => {
    setInvokeHandler("list_identities", () => { throw new Error("fixture load failure"); });
    const { pane } = begin({ identity_id: "shared" });
    await vi.waitFor(() => expect(view!.text()).toContain("Could not load SSH identities"));
    expect(calls()).toHaveLength(0);
    setInvokeHandler("list_identities", () => [identity()]);
    await view!.get('[role="alert"] button').trigger("click");
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(calls()[0][1].expectedUsername).toBe("root");
  });
  it("does not create a late session after closure during identity loading", async () => {
    let release!: (items: Identity[]) => void;
    setInvokeHandler("list_identities", () => new Promise<Identity[]>(resolve => { release = resolve; }));
    const { tabs, pane } = begin({ identity_id: "shared" });
    await vi.waitFor(() => expect(release).toBeDefined());
    tabs.closePane(pane.id);
    await nextTick();
    release([identity()]);
    await flushPromises();
    expect(calls()).toHaveLength(0);
    expect(tabs.tabs).toHaveLength(0);
  });
  it("publishes backend metadata only after success and never relabels the live session on edits", async () => {
    setInvokeHandler("list_identities", () => [identity()]);
    setInvokeHandler("connect_ssh", () => ({ username: "root", hostname: "backend.example.test", port: 2200 }));
    const { pane } = begin({ identity_id: "shared" });
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(view!.text()).toContain("root@backend.example.test:2200");
    useIdentitiesStore().identities = [identity({ username: "ops" })];
    await nextTick();
    expect(view!.text()).toContain("root@backend.example.test:2200");
  });
});

describe("ephemeral per-pane SSH passwords", () => {
  it.each([false, true])("prompts for effective password auth (linked identity=%s), without trimming or saving it", async linked => {
    setInvokeHandler("list_identities", () => linked ? [identity({ auth: passwordMethod })] : []);
    const { pane } = begin(linked ? { identity_id: "shared" } : { auth: passwordMethod });
    const input = await passwordInput();
    expect(document.activeElement).toBe(input.element);
    expect(view!.get('[role="region"]').text()).toContain(`${linked ? "root" : "deploy"}@atlas.example.test:22`);
    expect(calls()).toHaveLength(0);
    const oldInput = await submit(" fixture-only password ");
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(oldInput.value).toBe("");
    expect(calls()[0][1].password).toBe(" fixture-only password ");
    expect(calls()[0][1].expectedUsername).toBe(linked ? "root" : "deploy");
    expect(JSON.stringify([useHostsStore().$state, useIdentitiesStore().$state, useTabsStore().$state])).not.toContain("fixture-only password");
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "session_write")).toHaveLength(0);
    expect(view!.find('input[aria-label="SSH password"]').exists()).toBe(false);
  });
  it("does not prompt when an agent identity overrides password auth on the saved host", async () => {
    setInvokeHandler("list_identities", () => [identity()]);
    const { pane } = begin({ identity_id: "shared", auth: passwordMethod });
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(view!.find('input[aria-label="SSH password"]').exists()).toBe(false);
    expect(calls()[0][1].password).toBeNull();
  });
  it.each(["button", "escape"])("cancels through %s without dispatching or retaining a password", async method => {
    begin({ auth: passwordMethod });
    const input = await passwordInput();
    await input.setValue("fixture-cancelled");
    if (method === "escape") await input.trigger("keydown", { key: "Escape" });
    else await view!.get('button[type="button"]').trigger("click");
    await vi.waitFor(() => expect(view!.text()).toContain("Connection cancelled"));
    expect(input.element.value).toBe("");
    expect(calls()).toHaveLength(0);
    await view!.get('[role="alert"] button').trigger("click");
    expect((await passwordInput()).element.value).toBe("");
  });
  it("clears credentials and aborts a prompt when its pane is closed", async () => {
    const { tabs, pane } = begin({ auth: passwordMethod });
    const input = await passwordInput();
    await input.setValue("fixture-closed");
    tabs.closePane(pane.id);
    await flushPromises();
    expect(input.element.value).toBe("");
    expect(calls()).toHaveLength(0);
    expect(tabs.tabs).toHaveLength(0);
  });
  it("rejects identity edits during password entry instead of reusing credentials for a new account", async () => {
    setInvokeHandler("list_identities", () => [identity({ auth: passwordMethod })]);
    begin({ identity_id: "shared" });
    await passwordInput();
    useIdentitiesStore().identities = [identity({ username: "ops", auth: passwordMethod })];
    const input = await submit("fixture-changed");
    await vi.waitFor(() => expect(view!.text()).toContain("Connection settings changed"));
    expect(input.value).toBe("");
    expect(calls()).toHaveLength(0);
  });
  it("requests a fresh password after failure and after reconnect without remounting xterm", async () => {
    setInvokeHandler("connect_ssh", () => { throw new Error("authentication rejected by server"); });
    const { pane } = begin({ auth: passwordMethod });
    await passwordInput();
    const originalTerminal = view!.get(".xterm").element;
    await submit("fixture-first");
    await vi.waitFor(() => expect(view!.text()).toContain("authentication rejected"));
    setInvokeHandler("connect_ssh", args => ({ username: args.expectedUsername, hostname: args.host.hostname, port: args.host.port }));
    await view!.get('[role="alert"] button').trigger("click");
    expect((await passwordInput()).element.value).toBe("");
    await submit("fixture-second");
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    emitSessionClosed(pane.sessionId, "fixture disconnect");
    await nextTick();
    await view!.get('[role="alert"] button').trigger("click");
    expect((await passwordInput()).element.value).toBe("");
    await submit(""); // An intentionally empty password is different from cancellation.
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    expect(calls().map(([, args]) => args.password)).toEqual(["fixture-first", "fixture-second", ""]);
    expect(view!.get(".xterm").element).toBe(originalTerminal);
  });
});

describe("picker identity presentation", () => {
  it("displays/searches the effective username, not the stale saved-host fallback", async () => {
    useHostsStore().hosts = [host({ identity_id: "shared" })];
    setInvokeHandler("list_identities", () => [identity()]);
    const picker = mount(SessionPicker, { attachTo: document.body }); view = picker;
    await picker.vm.show();
    expect(picker.text()).toContain("root@atlas.example.test:22");
    expect(picker.text()).not.toContain("deploy@");
    await picker.get('[aria-label="Search sessions"]').setValue("root");
    expect(picker.find('[aria-label="Connect Atlas"]').exists()).toBe(true);
    await picker.get('[aria-label="Search sessions"]').setValue("deploy");
    expect(picker.find('[aria-label="Connect Atlas"]').exists()).toBe(false);
  });
  it("disables SSH during a delayed load while leaving local shells available", async () => {
    let release!: (items: Identity[]) => void;
    useHostsStore().hosts = [host({ identity_id: "shared" })];
    setInvokeHandler("list_identities", () => new Promise<Identity[]>(resolve => { release = resolve; }));
    const picker = mount(SessionPicker, { attachTo: document.body }); view = picker;
    const opened = picker.vm.show();
    await vi.waitFor(() => expect(release).toBeDefined());
    expect(picker.get<HTMLButtonElement>('[aria-label="Connect Atlas"]').element.disabled).toBe(true);
    expect(picker.get<HTMLButtonElement>('[aria-label="Open local shell"]').element.disabled).toBe(false);
    release([identity()]); await opened;
    expect(picker.get<HTMLButtonElement>('[aria-label="Connect Atlas"]').element.disabled).toBe(false);
  });
  it("offers retry rather than advertising fallback accounts after a failed load", async () => {
    useHostsStore().hosts = [host({ identity_id: "shared" })];
    setInvokeHandler("list_identities", () => { throw new Error("fixture load failure"); });
    const picker = mount(SessionPicker, { attachTo: document.body }); view = picker;
    await picker.vm.show();
    expect(picker.get<HTMLButtonElement>('[aria-label="Connect Atlas"]').element.disabled).toBe(true);
    expect(picker.text()).not.toContain("deploy@");
    setInvokeHandler("list_identities", () => [identity()]);
    await picker.get('[role="alert"] button').trigger("click");
    await flushPromises();
    expect(picker.text()).toContain("root@atlas.example.test:22");
    expect(picker.get<HTMLButtonElement>('[aria-label="Connect Atlas"]').element.disabled).toBe(false);
  });
});
