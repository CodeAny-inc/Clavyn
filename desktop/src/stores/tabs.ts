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
  const dragOverPaneId = ref<string | null>(null);
  const dragOverPosition = ref<DropPosition | null>(null);
  const activeTab = computed(() => tabs.value.find(t => t.id === activeTabId.value) ?? null);
  const activePane = computed(() => activeTab.value && activePaneId.value
    ? findPane(activeTab.value.tree, activePaneId.value) : null);
  function owningTab(id: string) { return tabs.value.find(t => findPane(t.tree, id)); }
  function firstPane(tree: PaneTree): Pane { return isPane(tree) ? tree : firstPane(tree.first); }
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
  function clearDragOver() { dragOverPaneId.value = null; dragOverPosition.value = null; }
  function endDrag() { draggedPaneId.value = null; clearDragOver(); }
  function setDragOver(id: string, position: DropPosition) {
    if (draggedPaneId.value === id) return;
    dragOverPaneId.value = id;
    dragOverPosition.value = position;
  }
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
    setActivePane(source.id);
    endDrag();
    paneFocusRequest.value = { paneId: source.id };
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
      if (focusedPanes.get(source.id) === id) focusedPanes.set(source.id, firstPane(source.tree).id);
    }
    target.tree = { id: paneId(), direction: "horizontal", ratio: 0.5, first: target.tree, second: pane };
    setActivePane(id);
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
    draggedPaneId, dragOverPaneId, dragOverPosition,
    newTab, closeTab, setActiveTab, setActivePane, splitPane, closePane,
    setPaneConnected, setPaneDisconnected, setPaneTitle, setRatio, firstPane,
    startDrag, endDrag, setDragOver, clearDragOver, dropPane, movePaneToTab, navigatePane, reorderTab };
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
