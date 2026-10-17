import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import WorkspaceView from "./WorkspaceView.vue";
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
  it("asks first, and cancelling leaves tabs and sessions alone", async () => {
    const { tabs, tab, pane } = connectedTab();
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);

    const view = await openWorkspace();

    expect(confirm).toHaveBeenCalledTimes(1);
    expect(tabs.tabs.map((t) => t.id)).toEqual([tab.id]);
    expect(pane.connected).toBe(true);
    expect(pane.closing).toBe(false);
    expect(calls("close_session")).toHaveLength(0);
    expect(view.emitted("switch-view")).toBeUndefined();
  });

  it("replaces the tabs and shows the terminal once confirmed", async () => {
    const { tabs } = connectedTab();
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);

    const view = await openWorkspace();

    expect(confirm).toHaveBeenCalledTimes(1);
    expect(calls("close_session")).toEqual([["close_session", { sessionId: "live-session" }]]);
    expect(tabs.tabs.map((t) => t.id)).toEqual(["workspace-tab"]);
    expect(view.emitted("switch-view")).toEqual([["terminal"]]);
  });

  it("opens without asking when nothing is connected", async () => {
    const confirm = vi.spyOn(window, "confirm");

    const view = await openWorkspace();

    expect(confirm).not.toHaveBeenCalled();
    expect(useTabsStore().tabs.map((t) => t.id)).toEqual(["workspace-tab"]);
    expect(view.emitted("switch-view")).toEqual([["terminal"]]);
  });
});
