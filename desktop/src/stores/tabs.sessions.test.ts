import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { collectPanes, isSplit, useTabsStore } from "./tabs";
import { getInvokeMock } from "../test/setup";
import type { Host } from "../types";
const host = (id: string): Host => ({ id, label: id, hostname: `${id}.example.test`, port: 22, username: "demo", auth: "agent", tags: [] });
const closes = () => getInvokeMock().mock.calls.filter(([command]) => command === "close_session");
beforeEach(() => setActivePinia(createPinia()));

describe("persistent terminal workspace", () => {
  it("splits a second host without changing the first session", () => {
    const store = useTabsStore();
    const tab = store.newTab(host("atlas"));
    const first = store.activePaneId!;
    store.setPaneConnected(first, "session-atlas");
    const second = store.splitPane(first, "horizontal", host("orion"))!;
    expect(collectPanes(store.tabs[0].tree).map(p => [p.hostId, p.sessionId])).toEqual([["atlas", "session-atlas"], ["orion", null]]);
    expect(store.activeTabId).toBe(tab.id);
    expect(store.activePaneId).toBe(second.id);
    expect(closes()).toEqual([]);
  });
  it("routes background connection and title callbacks to their owning pane", () => {
    const store = useTabsStore();
    store.newTab(host("atlas"));
    const first = store.activePaneId!;
    store.newTab(host("orion"));
    store.setPaneConnected(first, "late-atlas");
    store.setPaneTitle(first, "atlas: deploy");
    expect(collectPanes(store.tabs[0].tree)[0]).toMatchObject({ sessionId: "late-atlas", connected: true, title: "atlas: deploy" });
    expect(store.activePane?.hostId).toBe("orion");
  });
  it("remembers the focused split pane independently for each tab", () => {
    const store = useTabsStore();
    const first = store.newTab(host("atlas"));
    const pane = store.splitPane(store.activePaneId!, "vertical", host("orion"))!;
    store.newTab();
    store.setActiveTab(first.id);
    expect(store.activePaneId).toBe(pane.id);
  });
  it("swaps whole pane identities, not just their IDs", () => {
    const store = useTabsStore();
    store.newTab(host("atlas"));
    const first = store.activePaneId!;
    store.setPaneConnected(first, "atlas-session");
    const second = store.splitPane(first, "horizontal", host("orion"))!;
    store.setPaneConnected(second.id, "orion-session");
    store.startDrag(first); store.dropPane(second.id, "center");
    expect(collectPanes(store.tabs[0].tree).map(p => [p.id, p.hostId, p.sessionId])).toEqual([
      [second.id, "orion", "orion-session"], [first, "atlas", "atlas-session"],
    ]);
    expect(closes()).toEqual([]);
  });
  it.each(["top", "bottom", "left", "right"] as const)("moves a pane to %s without disconnecting", position => {
    const store = useTabsStore();
    store.newTab();
    const first = store.activePaneId!;
    store.setPaneConnected(first, "keep-me");
    const second = store.splitPane(first, "horizontal")!;
    store.startDrag(first); store.dropPane(second.id, position);
    expect(collectPanes(store.tabs[0].tree).find(p => p.id === first)?.sessionId).toBe("keep-me");
    expect(closes()).toEqual([]);
  });
  it("moves the sole pane of a tab into another tab without closing it", () => {
    const store = useTabsStore();
    const source = store.newTab(host("atlas"));
    const first = store.activePaneId!;
    store.setPaneConnected(first, "keep-me");
    const target = store.newTab(host("orion"));
    store.movePaneToTab(first, target.id);
    expect(store.tabs.some(t => t.id === source.id)).toBe(false);
    expect(collectPanes(store.tabs[0].tree)).toHaveLength(2);
    expect(store.activePaneId).toBe(first);
    expect(store.activePane?.sessionId).toBe("keep-me");
    expect(closes()).toEqual([]);
  });
  it("validates both endpoints before moving a pane", () => {
    const store = useTabsStore();
    store.newTab();
    const first = store.activePaneId!;
    store.splitPane(first, "horizontal");
    const before = JSON.stringify(store.tabs);
    store.movePaneToTab(first, "missing-tab");
    store.startDrag(first); store.dropPane("missing-pane", "left");
    expect(store.splitPane("missing-pane", "vertical")).toBeNull();
    expect(JSON.stringify(store.tabs)).toBe(before);
    expect(closes()).toEqual([]);
  });
  it("closes only the removed pane and preserves its sibling", () => {
    const store = useTabsStore();
    store.newTab();
    const first = store.activePaneId!;
    store.setPaneConnected(first, "keep-me");
    const second = store.splitPane(first, "horizontal")!;
    store.setPaneConnected(second.id, "close-me");
    store.closePane(second.id);
    expect(closes()).toEqual([["close_session", { sessionId: "close-me" }]]);
    expect(store.activePane?.sessionId).toBe("keep-me");
  });
  it("closes a late connection instead of resurrecting a removed pane", () => {
    const store = useTabsStore();
    store.newTab();
    const first = store.activePaneId!;
    store.closePane(first);
    store.setPaneConnected(first, "late-session");
    expect(store.tabs).toEqual([]);
    expect(closes()).toEqual([["close_session", { sessionId: "late-session" }]]);
  });
  it("updates a background split ratio safely", () => {
    const store = useTabsStore();
    store.newTab();
    store.splitPane(store.activePaneId!, "horizontal");
    const tree = store.tabs[0].tree;
    if (!isSplit(tree)) throw new Error("Expected split");
    store.newTab();
    store.setRatio(tree.id, 5);
    expect(tree.ratio).toBe(0.9);
    store.setRatio(tree.id, Number.NaN);
    expect(tree.ratio).toBe(0.9);
  });
});
