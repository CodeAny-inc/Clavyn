import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import SessionPicker from "./SessionPicker.vue";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { useVaultStore } from "../stores/vault";
import { useUiStore } from "../stores/ui";
import { emitTauriEvent, getInvokeMock, setInvokeHandler } from "../test/setup";
import type { AuthMethod, Host, Identity } from "../types";

const terminals = vi.hoisted(() => ({ items: [] as Array<{
  output: string[]; input: (data: string) => void;
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
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class { clearDecorations() {} onDidChangeResults() { return () => {}; } } }));

const passwordAuth: AuthMethod = { password: { credential_key: "fixture-only" } };
const identity: Identity = { id: "shared", label: "Shared", username: "root", auth: "agent", tags: [] };
const host = (overrides: Partial<Host> = {}): Host => ({ id: "atlas", label: "Atlas",
  hostname: "atlas.example.test", port: 22, username: "deploy", auth: "agent", tags: [], ...overrides });
let view: VueWrapper | undefined;
const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
function begin(saved: Host | null = host()) {
  useHostsStore().hosts = saved ? [saved] : [];
  const tab = useTabsStore().newTab(saved ?? undefined);
  const pane = collectPanes(tab.tree)[0];
  view = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });
  return pane;
}
function closeEvent(sessionId: string | null) {
  emitTauriEvent("session-closed", { session_id: sessionId, reason: "fixture EOF" });
}
async function submitPassword(value = "fixture-only password") {
  await vi.waitFor(() => expect(view!.find('input[aria-label="SSH password"]').exists()).toBe(true));
  const input = view!.get<HTMLInputElement>('input[aria-label="SSH password"]');
  await input.setValue(value);
  await view!.get("form").trigger("submit");
  return input.element;
}
async function unavailableIdentities(mode: "pending" | "failed") {
  let release!: (items: Identity[]) => void;
  setInvokeHandler("list_identities", () => {
    if (mode === "failed") throw new Error("fixture identity outage");
    return new Promise<Identity[]>(resolve => { release = resolve; });
  });
  const store = useIdentitiesStore();
  const loading = store.load();
  if (mode === "failed") await loading;
  return async () => { if (mode === "pending") release([identity]); await loading; };
}
beforeEach(() => {
  setActivePinia(createPinia());
  terminals.items.length = 0;
  document.body.innerHTML = "";
  useVaultStore().unlocked = true;
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.spyOn(HTMLDialogElement.prototype, "showModal").mockImplementation(function (this: HTMLDialogElement) { this.open = true; });
  vi.spyOn(HTMLDialogElement.prototype, "close").mockImplementation(function (this: HTMLDialogElement) { this.open = false; });
});
afterEach(() => { view?.unmount(); view = undefined; vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("write failure ownership", () => {
  for (const local of [false, true]) {
    it.each(["eof", "replacement", "closing", "disposed", "current"])(`isolates ${local ? "local" : "SSH"} rejection at %s`, async boundary => {
      const pane = begin(local ? null : host());
      await vi.waitFor(() => expect(pane.connected).toBe(true));
      const terminal = terminals.items[0];
      const original = view!.get(".xterm").element;
      const oldId = pane.sessionId;
      let reject!: (error: Error) => void;
      setInvokeHandler("session_write", () => new Promise<void>((_, no) => { reject = no; }));
      terminal.input("x");
      expect(calls("session_write")[0][1].sessionId).toBe(oldId);
      if (boundary === "eof" || boundary === "replacement") {
        closeEvent(oldId); await nextTick();
        if (boundary === "replacement") {
          await view!.get('[role="alert"] button').trigger("click");
          await vi.waitFor(() => expect(pane.connected).toBe(true));
          expect(pane.sessionId).not.toBe(oldId);
        }
      } else if (boundary === "closing") pane.closing = true;
      else if (boundary === "disposed") { view!.unmount(); view = undefined; }
      const before = [...terminal.output];
      const alertBefore = view?.findAll('[role="alert"]').map(alert => alert.text());
      reject(new Error("OLD_WRITE_FAILURE"));
      await flushPromises();
      if (boundary === "current") {
        expect(terminal.output.join("")).toContain("Write failed: Error: OLD_WRITE_FAILURE");
        expect(view!.get('[role="alert"]').text()).toContain("OLD_WRITE_FAILURE");
      } else {
        expect(terminal.output).toEqual(before);
        expect(view?.findAll('[role="alert"]').map(alert => alert.text())).toEqual(alertBefore);
      }
      if (view) expect(view.get(".xterm").element).toBe(original);
      expect(terminals.items).toHaveLength(1);
    });
  }
  it("ignores an old rejection while its replacement is still connecting", async () => {
    const pane = begin();
    await vi.waitFor(() => expect(pane.connected).toBe(true));
    let reject!: (error: Error) => void;
    setInvokeHandler("session_write", () => new Promise<void>((_, no) => { reject = no; }));
    terminals.items[0].input("x");
    closeEvent(pane.sessionId); await nextTick();
    let finish!: () => void;
    setInvokeHandler("connect_ssh", () => new Promise<void>(resolve => { finish = resolve; }));
    await view!.get('[role="alert"] button').trigger("click");
    await vi.waitFor(() => expect(finish).toBeDefined());
    reject(new Error("OLD_WRITE_FAILURE")); await flushPromises();
    expect(view!.find('[role="alert"]').exists()).toBe(false);
    expect(terminals.items[0].output.join("")).not.toContain("OLD_WRITE_FAILURE");
    finish(); await vi.waitFor(() => expect(pane.connected).toBe(true));
  });
});

describe("direct-host identity fault isolation", () => {
  for (const mode of ["pending", "failed"] as const) {
    it.each(["agent", "publickey", "password"])(`connects/reconnects direct %s with ${mode} identities`, async method => {
      const finishLoad = await unavailableIdentities(mode);
      try {
        const pane = begin(host({ identity_id: null, auth: method === "password" ? passwordAuth : method as AuthMethod, key_id: "fixture-key" }));
        if (method === "password") expect((await submitPassword()).value).toBe("");
        await vi.waitFor(() => expect(pane.connected).toBe(true));
        const oldId = pane.sessionId;
        expect(useIdentitiesStore().loaded).toBe(false);
        expect(calls("list_identities")).toHaveLength(1);
        closeEvent(oldId); await nextTick();
        await view!.get('[role="alert"] button').trigger("click");
        if (method === "password") expect((await submitPassword()).value).toBe("");
        await vi.waitFor(() => expect(pane.connected).toBe(true));
        expect(pane.sessionId).not.toBe(oldId);
        expect(calls("connect_ssh")).toHaveLength(2);
        expect(calls("connect_ssh").map(([, args]) => args.expectedUsername)).toEqual(["deploy", "deploy"]);
        expect(calls("session_write")).toHaveLength(0);
        expect(calls("list_identities")).toHaveLength(1);
        expect(terminals.items).toHaveLength(1);
      } finally { await finishLoad(); }
    });
    it(`enables, labels and searches only direct accounts while identities are ${mode}`, async () => {
      const finishLoad = await unavailableIdentities(mode);
      try {
        useHostsStore().hosts = [host(), host({ id: "orion", label: "Orion", identity_id: "shared", username: "stale" })];
        const picker = mount(SessionPicker, { attachTo: document.body }); view = picker;
        void picker.vm.show(); await flushPromises();
        expect(picker.get<HTMLButtonElement>('[aria-label="Connect Atlas"]').element.disabled).toBe(false);
        expect(picker.get('[aria-label="Connect Atlas"]').text()).toContain("deploy@atlas.example.test:22");
        expect(picker.get<HTMLButtonElement>('[aria-label="Connect Orion"]').element.disabled).toBe(true);
        await picker.get('[aria-label="Search sessions"]').setValue("stale");
        expect(picker.find('[aria-label="Connect Orion"]').exists()).toBe(false);
        await picker.get('[aria-label="Search sessions"]').setValue("deploy");
        await picker.get('[aria-label="Search sessions"]').trigger("keydown", { key: "Enter" });
        expect(useTabsStore().activePane?.hostId).toBe("atlas");
      } finally { await finishLoad(); }
    });
  }
  it.each([false, true])("rechecks a new identity link after vault unlock (load succeeds=%s)", async succeeds => {
    useVaultStore().unlocked = false;
    setInvokeHandler("list_identities", () => {
      if (!succeeds) throw new Error("fixture identity outage");
      return [identity];
    });
    const pane = begin(host({ auth: "publickey", key_id: "fixture-key" }));
    await vi.waitFor(() => expect(useUiStore().showVaultUnlockModal).toBe(true));
    expect(calls("list_identities")).toHaveLength(0);
    useHostsStore().hosts = [host({ identity_id: "shared" })];
    useVaultStore().unlocked = true;
    useUiStore().resolveVaultUnlock(true);
    if (succeeds) {
      await vi.waitFor(() => expect(pane.connected).toBe(true));
      expect(calls("connect_ssh")[0][1].expectedUsername).toBe("root");
    } else {
      await vi.waitFor(() => expect(view!.text()).toContain("Could not load SSH identities"));
      expect(calls("connect_ssh")).toHaveLength(0);
    }
    expect(calls("list_identities")).toHaveLength(1);
  });
  it("rejects a new identity link during a direct password prompt without reusing the password", async () => {
    const finishLoad = await unavailableIdentities("failed");
    begin(host({ auth: passwordAuth }));
    await vi.waitFor(() => expect(view!.find('input[aria-label="SSH password"]').exists()).toBe(true));
    useHostsStore().hosts = [host({ auth: passwordAuth, identity_id: "shared" })];
    expect((await submitPassword()).value).toBe("");
    await vi.waitFor(() => expect(view!.text()).toContain("Connection settings changed"));
    expect(calls("connect_ssh")).toHaveLength(0);
    await finishLoad();
  });
});
