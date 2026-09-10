import { defineStore } from "pinia";
import { ref, computed } from "vue";
import * as api from "../api";
import type { Host } from "../types";

export interface Pane {
  id: string;
  sessionId: string | null;
  hostId: string | null;
  terminalType: "ssh" | "local";
  title: string;
  connected: boolean;
  closing: boolean;
  /** false only for restored SSH panes in a workspace with auto_connect disabled. */
  autoConnect?: boolean;
}
export interface SplitNode {
  id: string;
  direction: "horizontal" | "vertical";
  ratio: number;
  first: PaneTree;
  second: PaneTree;
}
export type PaneTree = Pane | SplitNode;
export interface Tab { id: string; title: string; tree: PaneTree }
export type DropPosition = "top" | "bottom" | "left" | "right" | "center";
export const isPane = (node: PaneTree): node is Pane => !("direction" in node);
export const isSplit = (node: PaneTree): node is SplitNode => "direction" in node;
export const paneId = (): string => crypto.randomUUID();
function makePane(host?: Host): Pane {
  return { id: paneId(), sessionId: null, hostId: host?.id ?? null,
    terminalType: host ? "ssh" : "local", title: host?.label ?? "Local Terminal",
    connected: false, closing: false };
}
export const useTabsStore = defineStore("tabs", () => {
  const tabs = ref<Tab[]>([]);
  const activeTabId = ref<string | null>(null);
  const activePaneId = ref<string | null>(null);
  // A fresh request also notifies an already-active pane after a DOM move.
  const paneFocusRequest = ref<{ paneId: string } | null>(null);
  const focusedPanes = new Map<string, string>();
  const draggedPaneId = ref<string | null>(null);
  // A whole tab dragged from the strip — distinct from a pane grip drag. Its
  // drop target is a pane of the active tab (split/swap) or a strip position.
  const draggedTabId = ref<string | null>(null);
  const dragOverPaneId = ref<string | null>(null);
  const dragOverPosition = ref<DropPosition | null>(null);
  // Highlighted tab while a pane or tab is dragged over it, so the tab strip
  // can show a "drop here" affordance like Termius workspaces.
  const dragOverTabId = ref<string | null>(null);
  const activeTab = computed(() => tabs.value.find(t => t.id === activeTabId.value) ?? null);
  const activePane = computed(() => activeTab.value && activePaneId.value
    ? findPane(activeTab.value.tree, activePaneId.value) : null);
  function owningTab(id: string) { return tabs.value.find(t => findPane(t.tree, id)); }
  function firstPane(tree: PaneTree): Pane { return isPane(tree) ? tree : firstPane(tree.first); }
  // A tab is named after its first pane; re-sync the title after any tree
  // change so tooltips, close labels and "Move to…" entries keep identifying
  // the terminal the tab actually shows.
  function syncTabTitle(tab: Tab) { tab.title = firstPane(tab.tree).title; }
  function setActivePane(id: string) {
    const tab = owningTab(id);
    if (!tab) return;
    activeTabId.value = tab.id;
    activePaneId.value = id;
    focusedPanes.set(tab.id, id);
  }
  function setActiveTab(id: string) {
    const tab = tabs.value.find(t => t.id === id);
    if (!tab) return;
    const remembered = focusedPanes.get(id);
    setActivePane(remembered && findPane(tab.tree, remembered) ? remembered : firstPane(tab.tree).id);
  }
  function newTab(host?: Host): Tab {
    const pane = makePane(host);
    const tab = { id: paneId(), title: host?.label ?? "Local", tree: pane };
    tabs.value.push(tab);
    setActivePane(pane.id);
    return tab;
  }
  function closeSession(pane: Pane) {
    pane.closing = true;
    if (pane.sessionId) void api.closeSession(pane.sessionId).catch(() => {});
  }
  function closeTab(id: string) {
    const index = tabs.value.findIndex(t => t.id === id);
    if (index < 0) return;
    collectPanes(tabs.value[index].tree).forEach(closeSession);
    tabs.value.splice(index, 1);
    focusedPanes.delete(id);
    if (activeTabId.value === id) {
      const next = tabs.value[Math.min(index, tabs.value.length - 1)];
      if (next) setActiveTab(next.id);
      else { activeTabId.value = null; activePaneId.value = null; }
    }
  }
  function splitPane(id: string, direction: SplitNode["direction"], host?: Host): Pane | null {
    const tab = owningTab(id);
    if (!tab) return null;
    const pane = makePane(host);
    tab.tree = replace(tab.tree, id, old => ({ id: paneId(), direction, ratio: 0.5, first: old, second: pane }));
    setActivePane(pane.id);
    return pane;
  }
  function closePane(id: string) {
    const tab = owningTab(id);
    if (!tab) return;
    if (isPane(tab.tree)) { closeTab(tab.id); return; }
    const pane = findPane(tab.tree, id)!;
    const sibling = findSibling(tab.tree, id);
    closeSession(pane);
    tab.tree = detach(tab.tree, id);
    syncTabTitle(tab);
    if (focusedPanes.get(tab.id) === id) {
      const next = firstPane(sibling ?? tab.tree).id;
      focusedPanes.set(tab.id, next);
      if (activeTabId.value === tab.id) setActivePane(next);
    }
  }
  // Connection callbacks may arrive while a different tab is visible.
  function setPaneConnected(id: string, sessionId: string) {
    const tab = owningTab(id);
    const pane = tab && findPane(tab.tree, id);
    if (!pane || pane.closing) {
      void api.closeSession(sessionId).catch(() => {});
      return;
    }
    pane.sessionId = sessionId;
    pane.connected = true;
  }
  function setPaneDisconnected(id: string) {
    const tab = owningTab(id);
    const pane = tab && findPane(tab.tree, id);
    if (pane) pane.connected = false;
  }
  function setPaneTitle(id: string, title: string) {
    const tab = owningTab(id);
    const pane = tab && findPane(tab.tree, id);
    if (pane) pane.title = title;
  }
  function setRatio(id: string, ratio: number) {
    if (!Number.isFinite(ratio)) return;
    for (const tab of tabs.value) {
      const node = findSplit(tab.tree, id);
      if (node) { node.ratio = Math.max(0.1, Math.min(0.9, ratio)); return; }
    }
  }
  function startDrag(id: string) { draggedPaneId.value = id; }
  function startTabDrag(id: string) { draggedTabId.value = id; }
  function clearDragOver() { dragOverPaneId.value = null; dragOverPosition.value = null; dragOverTabId.value = null; }
  function endDrag() { draggedPaneId.value = null; draggedTabId.value = null; clearDragOver(); }
  function setDragOver(id: string, position: DropPosition) {
    if (!draggedPaneId.value && !draggedTabId.value) return;
    if (draggedPaneId.value === id) return;
    // A tab cannot be dropped onto one of its own panes.
    if (draggedTabId.value && owningTab(id)?.id === draggedTabId.value) return;
    dragOverPaneId.value = id;
    dragOverPosition.value = position;
  }
  // Highlight a tab as a drop target. Only meaningful while a pane or a tab is dragged.
  function setDragOverTab(id: string) {
    if (!draggedPaneId.value && !draggedTabId.value) return;
    if (draggedTabId.value === id) return;
    dragOverTabId.value = id;
  }
  function clearDragOverTab() { dragOverTabId.value = null; }
  function dropPane(targetId: string, position: DropPosition) {
    const id = draggedPaneId.value;
    const tab = id ? owningTab(id) : undefined;
    const source = tab && id ? findPane(tab.tree, id) : null;
    const target = tab ? findPane(tab.tree, targetId) : null;
    if (!tab || !source || !target || source.id === targetId) { endDrag(); return; }
    if (position === "center") {
      tab.tree = swap(tab.tree, source, target);
    } else {
      const direction = position === "left" || position === "right" ? "horizontal" : "vertical";
      const before = position === "left" || position === "top";
      tab.tree = replace(detach(tab.tree, source.id), targetId, old => ({
        id: paneId(), direction, ratio: 0.5,
        first: before ? source : old, second: before ? old : source,
      }));
    }
    syncTabTitle(tab);
    setActivePane(source.id);
    endDrag();
    paneFocusRequest.value = { paneId: source.id };
  }
  // Drop a dragged tab onto a pane of another tab: edge positions split the
  // target pane with the dragged tab's tree; center swaps a single-pane tab
  // with the target pane or merges a multi-pane tab into the target tab.
  function dropTabOnPane(sourceTabId: string, targetPaneId: string, position: DropPosition) {
    const source = tabs.value.find(t => t.id === sourceTabId);
    const target = owningTab(targetPaneId);
    if (!source || !target || source.id === target.id) { endDrag(); return; }
    const targetPane = findPane(target.tree, targetPaneId)!;
    if (position === "center" && isPane(source.tree)) {
      // Swap the two panes across their tabs, keeping each pane object intact.
      const moved = source.tree;
      target.tree = replace(target.tree, targetPaneId, () => moved);
      source.tree = targetPane;
      syncTabTitle(source);
      syncTabTitle(target);
      focusedPanes.set(source.id, targetPane.id);
      setActivePane(moved.id);
      paneFocusRequest.value = { paneId: moved.id };
    } else {
      const moved = source.tree;
      tabs.value = tabs.value.filter(t => t.id !== source.id);
      focusedPanes.delete(source.id);
      if (position === "center") {
        // A multi-pane tab has no single pane to swap; merge its split tree.
        target.tree = { id: paneId(), direction: "horizontal", ratio: 0.5, first: target.tree, second: moved };
      } else {
        const direction = position === "left" || position === "right" ? "horizontal" : "vertical";
        const before = position === "left" || position === "top";
        target.tree = replace(target.tree, targetPaneId, old => ({
          id: paneId(), direction, ratio: 0.5,
          first: before ? moved : old, second: before ? old : moved,
        }));
      }
      syncTabTitle(target);
      const focus = firstPane(moved).id;
      setActivePane(focus);
      paneFocusRequest.value = { paneId: focus };
    }
    endDrag();
  }
  function movePaneToTab(id: string, targetTabId: string) {
    const source = owningTab(id);
    const target = tabs.value.find(t => t.id === targetTabId);
    if (!source || !target || source.id === target.id) return;
    const pane = findPane(source.tree, id)!;
    if (isPane(source.tree)) {
      tabs.value = tabs.value.filter(t => t.id !== source.id);
      focusedPanes.delete(source.id);
    } else {
      source.tree = detach(source.tree, id);
      syncTabTitle(source);
      if (focusedPanes.get(source.id) === id) focusedPanes.set(source.id, firstPane(source.tree).id);
    }
    target.tree = { id: paneId(), direction: "horizontal", ratio: 0.5, first: target.tree, second: pane };
    syncTabTitle(target);
    setActivePane(id);
    endDrag();
    paneFocusRequest.value = { paneId: id };
  }
  // Pull a pane out of its current tab into a fresh tab of its own. The inverse
  // of movePaneToTab: a Termius "move a session back to a separate tab" gesture.
  // The pane object (and its live session) travels intact, so the connection is
  // never interrupted. The new tab is inserted after the source tab.
  function extractPaneToNewTab(id: string) {
    const source = owningTab(id);
    if (!source) return;
    const pane = findPane(source.tree, id);
    if (!pane) return;
    const insertAt = tabs.value.findIndex(t => t.id === source.id) + 1;
    if (isPane(source.tree)) {
      tabs.value = tabs.value.filter(t => t.id !== source.id);
      focusedPanes.delete(source.id);
    } else {
      source.tree = detach(source.tree, id);
      syncTabTitle(source);
      if (focusedPanes.get(source.id) === id) focusedPanes.set(source.id, firstPane(source.tree).id);
    }
    const tab = { id: paneId(), title: pane.title, tree: pane as PaneTree };
    tabs.value.splice(Math.min(insertAt, tabs.value.length), 0, tab);
    setActivePane(id);
    endDrag();
    paneFocusRequest.value = { paneId: id };
  }
  function navigatePane(direction: "up" | "down" | "left" | "right") {
    if (!activeTab.value || !activePaneId.value) return;
    const panes = collectPanes(activeTab.value.tree);
    const index = panes.findIndex(p => p.id === activePaneId.value);
    const next = panes[index + (direction === "right" || direction === "down" ? 1 : -1)];
    if (next) setActivePane(next.id);
  }
  function reorderTab(from: number, to: number) {
    if (from === to || from < 0 || to < 0 || from >= tabs.value.length || to >= tabs.value.length) return;
    const [tab] = tabs.value.splice(from, 1);
    tabs.value.splice(to, 0, tab);
  }
  return { tabs, activeTabId, activePaneId, activeTab, activePane, paneFocusRequest,
    draggedPaneId, draggedTabId, dragOverPaneId, dragOverPosition, dragOverTabId,
    newTab, closeTab, setActiveTab, setActivePane, splitPane, closePane,
    setPaneConnected, setPaneDisconnected, setPaneTitle, setRatio, firstPane, owningTab,
    startDrag, startTabDrag, endDrag, setDragOver, clearDragOver, setDragOverTab, clearDragOverTab,
    dropPane, dropTabOnPane, movePaneToTab, extractPaneToNewTab, navigatePane, reorderTab };
});
function replace(tree: PaneTree, id: string, change: (pane: Pane) => PaneTree): PaneTree {
  if (isPane(tree)) return tree.id === id ? change(tree) : tree;
  return { ...tree, first: replace(tree.first, id, change), second: replace(tree.second, id, change) };
}
// Detaching is structural: only closePane/closeTab close sessions.
function detach(tree: PaneTree, id: string): PaneTree {
  if (isPane(tree)) return tree;
  if (isPane(tree.first) && tree.first.id === id) return tree.second;
  if (isPane(tree.second) && tree.second.id === id) return tree.first;
  return { ...tree, first: detach(tree.first, id), second: detach(tree.second, id) };
}
function findPane(tree: PaneTree, id: string): Pane | null {
  if (isPane(tree)) return tree.id === id ? tree : null;
  return findPane(tree.first, id) ?? findPane(tree.second, id);
}
function findSplit(tree: PaneTree, id: string): SplitNode | null {
  if (isPane(tree)) return null;
  return tree.id === id ? tree : findSplit(tree.first, id) ?? findSplit(tree.second, id);
}
function findSibling(tree: PaneTree, id: string): PaneTree | null {
  if (isPane(tree)) return null;
  if (isPane(tree.first) && tree.first.id === id) return tree.second;
  if (isPane(tree.second) && tree.second.id === id) return tree.first;
  return findSibling(tree.first, id) ?? findSibling(tree.second, id);
}
// Move the full object, not just its ID, so host/session/terminal identity stays intact.
function swap(tree: PaneTree, a: Pane, b: Pane): PaneTree {
  if (isPane(tree)) return tree.id === a.id ? b : tree.id === b.id ? a : tree;
  return { ...tree, first: swap(tree.first, a, b), second: swap(tree.second, a, b) };
}
export function collectPanes(tree: PaneTree): Pane[] {
  return isPane(tree) ? [tree] : [...collectPanes(tree.first), ...collectPanes(tree.second)];
}
