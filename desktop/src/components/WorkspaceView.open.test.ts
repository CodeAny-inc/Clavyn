import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import WorkspaceView from "./WorkspaceView.vue";
import { answerConfirmation, pendingConfirmation } from "../composables/useConfirm";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host, Workspace } from "../types";

const host: Host = {
  id: "atlas", label: "Atlas", hostname: "atlas.example.test", port: 22, username: "deploy",
  auth: "agent", tags: [],
};
const workspace: Workspace = {
  id: "workspace-1", name: "Ops", description: null, color: null, icon: null,
  host_ids: [host.id], auto_connect: false,
  tabs: [{ id: "workspace-tab", title: "Atlas tab", layout: { type: "pane", host_id: host.id, terminal_type: "ssh" } }],
};
const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  setInvokeHandler("list_hosts", () => [host]);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("list_workspaces", () => [workspace]);
});
afterEach(() => {
  wrapper?.unmount(); wrapper = undefined;
  vi.restoreAllMocks();
});

function connectedTab() {
  const tabs = useTabsStore();
  const tab = tabs.newTab(host);
  const pane = collectPanes(tab.tree)[0];
  tabs.setPaneConnected(pane.id, "live-session");
  return { tabs, tab, pane };
}

async function openWorkspace() {
  wrapper = mount(WorkspaceView, { attachTo: document.body });
  await flushPromises();
  await wrapper.get('[aria-label="Restore workspace"]').trigger("click");
  await flushPromises();
  return wrapper;
}

describe("opening a workspace over live sessions", () => {
  it("leaves tabs and sessions alone until answered, and again once declined", async () => {
    const { tabs, tab, pane } = connectedTab();

    const view = await openWorkspace();

    // The question is on screen and nothing has been given up yet: the answer
    // arrives later than the click, as it does in the app.
    expect(pendingConfirmation.value?.message).toContain('Open workspace "Ops"?');
    expect(tabs.tabs.map((t) => t.id)).toEqual([tab.id]);
    expect(pane.connected).toBe(true);
    expect(calls("close_session")).toHaveLength(0);
    expect(view.emitted("switch-view")).toBeUndefined();

    answerConfirmation(false);
    await flushPromises();

    expect(tabs.tabs.map((t) => t.id)).toEqual([tab.id]);
    expect(pane.connected).toBe(true);
    expect(pane.closing).toBe(false);
    expect(calls("close_session")).toHaveLength(0);
    expect(view.emitted("switch-view")).toBeUndefined();
  });

  it("replaces the tabs and shows the terminal only once confirmed", async () => {
    const { tabs, tab } = connectedTab();

    const view = await openWorkspace();
    expect(tabs.tabs.map((t) => t.id)).toEqual([tab.id]);
    expect(calls("close_session")).toHaveLength(0);

    answerConfirmation(true);
    await flushPromises();

    expect(calls("close_session")).toEqual([["close_session", { sessionId: "live-session" }]]);
    expect(tabs.tabs.map((t) => t.id)).toEqual(["workspace-tab"]);
    expect(view.emitted("switch-view")).toEqual([["terminal"]]);
  });

  it("opens without asking when nothing is connected", async () => {
    const view = await openWorkspace();

    expect(pendingConfirmation.value).toBeNull();
    expect(useTabsStore().tabs.map((t) => t.id)).toEqual(["workspace-tab"]);
    expect(view.emitted("switch-view")).toEqual([["terminal"]]);
  });
});
