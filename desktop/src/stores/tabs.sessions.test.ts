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

describe("drag a pane between tabs (Termius-style split-on-drop)", () => {
  it("extracts a split pane into its own tab without closing its session", () => {
    const store = useTabsStore();
    store.newTab(host("atlas"));
    const first = store.activePaneId!;
    store.setPaneConnected(first, "keep-me");
    store.splitPane(first, "horizontal", host("orion"));
    store.startDrag(first);
    store.extractPaneToNewTab(first);
    // The pane keeps its live session and is now the sole pane of a new tab.
    expect(store.tabs).toHaveLength(2);
    expect(collectPanes(store.tabs[1].tree)).toEqual([expect.objectContaining({ id: first, sessionId: "keep-me", connected: true })]);
    expect(store.activeTabId).toBe(store.tabs[1].id);
    expect(store.activePaneId).toBe(first);
    expect(closes()).toEqual([]);
  });
  it("extracts the sole pane of a tab by removing the source tab", () => {
    const store = useTabsStore();
    store.newTab(host("atlas"));
    const first = store.activePaneId!;
    store.setPaneConnected(first, "keep-me");
    store.newTab(host("orion"));
    store.startDrag(first);
    store.extractPaneToNewTab(first);
    // The atlas pane left its (sole-pane) source tab, which is now gone.
    expect(store.tabs).toHaveLength(2);
    expect(store.tabs.some(t => collectPanes(t.tree).some(p => p.id === first && p.sessionId === "keep-me"))).toBe(true);
    expect(closes()).toEqual([]);
  });
  it("ignores extract for an unknown pane id", () => {
    const store = useTabsStore();
    store.newTab();
    const before = JSON.stringify(store.tabs);
    store.extractPaneToNewTab("missing-pane");
    expect(JSON.stringify(store.tabs)).toBe(before);
    expect(closes()).toEqual([]);
  });
  it("clears the tab drop highlight and drag state after a drop", () => {
    const store = useTabsStore();
    store.newTab(host("atlas"));
    const first = store.activePaneId!;
    store.newTab(host("orion"));
    store.startDrag(first);
    store.setDragOverTab(store.tabs[1].id);
    expect(store.dragOverTabId).toBe(store.tabs[1].id);
    store.movePaneToTab(first, store.tabs[1].id);
    expect(store.draggedPaneId).toBeNull();
    expect(store.dragOverTabId).toBeNull();
    expect(closes()).toEqual([]);
  });
  it("setDragOverTab is a no-op when no pane is being dragged", () => {
    const store = useTabsStore();
    store.newTab();
    store.setDragOverTab(store.tabs[0].id);
    expect(store.dragOverTabId).toBeNull();
  });
});

describe("drag a tab onto a terminal pane (Termius-style tab drop)", () => {
  it.each(["top", "bottom", "left", "right"] as const)("splits the target pane %s with the dragged tab", position => {
    const store = useTabsStore();
    const target = store.newTab(host("atlas"));
    const targetPane = store.activePaneId!;
    store.setPaneConnected(targetPane, "atlas-session");
    const source = store.newTab(host("orion"));
    const sourcePane = store.activePaneId!;
    store.setPaneConnected(sourcePane, "orion-session");
    store.startTabDrag(source.id);
    store.dropTabOnPane(source.id, targetPane, position);
    expect(store.tabs).toHaveLength(1);
    const tree = store.tabs[0].tree;
    expect(isSplit(tree)).toBe(true);
    if (!isSplit(tree)) throw new Error("Expected split");
    const horizontal = position === "left" || position === "right";
    const before = position === "left" || position === "top";
    expect(tree.direction).toBe(horizontal ? "horizontal" : "vertical");
    const first = before ? sourcePane : targetPane;
    const second = before ? targetPane : sourcePane;
    expect(collectPanes(tree).map(p => [p.id, p.sessionId])).toEqual([
      [first, first === sourcePane ? "orion-session" : "atlas-session"],
      [second, second === sourcePane ? "orion-session" : "atlas-session"],
    ]);
    expect(store.activeTabId).toBe(store.tabs[0].id);
    expect(store.activePaneId).toBe(sourcePane);
    expect(closes()).toEqual([]);
    expect(store.draggedTabId).toBeNull();
  });
  it("swaps two panes across tabs on a center drop", () => {
    const store = useTabsStore();
    const target = store.newTab(host("atlas"));
    const targetPane = store.activePaneId!;
    store.setPaneConnected(targetPane, "atlas-session");
    const source = store.newTab(host("orion"));
    const sourcePane = store.activePaneId!;
    store.setPaneConnected(sourcePane, "orion-session");
    store.startTabDrag(source.id);
    store.dropTabOnPane(source.id, targetPane, "center");
    expect(store.tabs).toHaveLength(2);
    expect(collectPanes(store.tabs[0].tree).map(p => [p.id, p.sessionId])).toEqual([[sourcePane, "orion-session"]]);
    expect(collectPanes(store.tabs[1].tree).map(p => [p.id, p.sessionId])).toEqual([[targetPane, "atlas-session"]]);
    expect(store.tabs[0].title).toBe("orion");
    expect(store.tabs[1].title).toBe("atlas");
    expect(store.activeTabId).toBe(store.tabs[0].id);
    expect(store.activePaneId).toBe(sourcePane);
    expect(closes()).toEqual([]);
  });
  it("moves a multi-pane tab's whole split tree on an edge drop", () => {
    const store = useTabsStore();
    const target = store.newTab(host("atlas"));
    const targetPane = store.activePaneId!;
    const source = store.newTab();
    const first = store.activePaneId!;
    store.setPaneConnected(first, "keep-a");
    const second = store.splitPane(first, "horizontal", host("orion"))!;
    store.setPaneConnected(second.id, "keep-b");
    store.startTabDrag(source.id);
    store.dropTabOnPane(source.id, targetPane, "left");
    expect(store.tabs).toHaveLength(1);
    const tree = store.tabs[0].tree;
    if (!isSplit(tree)) throw new Error("Expected split");
    expect(tree.direction).toBe("horizontal");
    expect(collectPanes(tree).map(p => p.sessionId)).toEqual(["keep-a", "keep-b", null]);
    expect(collectPanes(tree).map(p => p.id)).toEqual([first, second.id, targetPane]);
    expect(store.activePaneId).toBe(first);
    expect(closes()).toEqual([]);
  });
  it("merges a multi-pane tab at the root on a center drop", () => {
    const store = useTabsStore();
    const target = store.newTab(host("atlas"));
    const targetPane = store.activePaneId!;
    const source = store.newTab();
    const first = store.activePaneId!;
    const second = store.splitPane(first, "vertical")!;
    store.startTabDrag(source.id);
    store.dropTabOnPane(source.id, targetPane, "center");
    expect(store.tabs).toHaveLength(1);
    const tree = store.tabs[0].tree;
    if (!isSplit(tree)) throw new Error("Expected split");
    expect(tree.direction).toBe("horizontal");
    expect(collectPanes(tree).map(p => p.id)).toEqual([targetPane, first, second.id]);
    expect(closes()).toEqual([]);
  });
  it("keeps tab titles naming the first pane after structural moves", () => {
    const store = useTabsStore();
    // closePane: removing the first of two panes renames the tab.
    const single = store.newTab(host("atlas"));
    const first = store.activePaneId!;
    store.splitPane(first, "horizontal", host("orion"));
    store.closePane(first);
    expect(store.tabs.find(t => t.id === single.id)!.title).toBe("orion");
    // movePaneToTab: the source tab keeps the pane that stays behind.
    const multi = store.newTab(host("atlas"));
    const movedOut = store.activePaneId!;
    store.splitPane(movedOut, "horizontal", host("orion"));
    const dest = store.newTab(host("local"));
    store.movePaneToTab(movedOut, dest.id);
    expect(store.tabs.find(t => t.id === multi.id)!.title).toBe("orion");
    // dropPane edge drop: landing before the first pane renames the tab.
    const swapTab = store.newTab(host("atlas"));
    const a = store.activePaneId!;
    const b = store.splitPane(a, "horizontal", host("orion"))!;
    store.startDrag(b.id);
    store.dropPane(a, "left");
    expect(store.tabs.find(t => t.id === swapTab.id)!.title).toBe("orion");
  });
  it("ignores a tab dropped onto a pane of its own tab", () => {
    const store = useTabsStore();
    const tab = store.newTab();
    const pane = store.activePaneId!;
    store.splitPane(pane, "horizontal");
    const before = JSON.stringify(store.tabs);
    store.startTabDrag(tab.id);
    store.dropTabOnPane(tab.id, pane, "left");
    store.dropTabOnPane(tab.id, pane, "center");
    expect(JSON.stringify(store.tabs)).toBe(before);
    expect(store.draggedTabId).toBeNull();
    expect(closes()).toEqual([]);
  });
  it("ignores unknown tab or pane ids", () => {
    const store = useTabsStore();
    store.newTab();
    const pane = store.activePaneId!;
    const before = JSON.stringify(store.tabs);
    store.startTabDrag("missing-tab");
    store.dropTabOnPane("missing-tab", pane, "left");
    store.startTabDrag(store.tabs[0].id);
    store.dropTabOnPane(store.tabs[0].id, "missing-pane", "left");
    expect(JSON.stringify(store.tabs)).toBe(before);
    expect(closes()).toEqual([]);
  });
  it("does not highlight panes owned by the dragged tab", () => {
    const store = useTabsStore();
    const tab = store.newTab();
    const own = store.activePaneId!;
    const other = store.newTab();
    const otherPane = store.activePaneId!;
    store.startTabDrag(tab.id);
    store.setDragOver(own, "left");
    expect(store.dragOverPaneId).toBeNull();
    store.setDragOver(otherPane, "right");
    expect(store.dragOverPaneId).toBe(otherPane);
    expect(store.dragOverPosition).toBe("right");
    store.setDragOverTab(other.id);
    expect(store.dragOverTabId).toBe(other.id);
    store.setDragOverTab(tab.id);
    expect(store.dragOverTabId).toBe(other.id);
    store.endDrag();
    expect(store.draggedTabId).toBeNull();
    expect(store.dragOverPaneId).toBeNull();
    expect(store.dragOverTabId).toBeNull();
  });
  it("reorders tabs by dragged index and target index", () => {
    const store = useTabsStore();
    const a = store.newTab();
    const b = store.newTab();
    const c = store.newTab();
    store.reorderTab(0, 2);
    expect(store.tabs.map(t => t.id)).toEqual([b.id, c.id, a.id]);
    store.reorderTab(2, 0);
    expect(store.tabs.map(t => t.id)).toEqual([a.id, b.id, c.id]);
    store.reorderTab(0, 0);
    store.reorderTab(-1, 1);
    store.reorderTab(0, 9);
    expect(store.tabs.map(t => t.id)).toEqual([a.id, b.id, c.id]);
  });
});
