<script setup lang="ts">
import { computed, ref, watch, onMounted, onBeforeUnmount, type CSSProperties } from "vue";
import { useTabsStore, isPane, type Pane, type PaneTree, type SplitNode } from "../stores/tabs";
import { useUiStore } from "../stores/ui";
import TerminalPane from "./TerminalPane.vue";

const props = defineProps<{ visible: boolean }>();
const emit = defineEmits<{ "request-session": [paneId: string, direction: "horizontal" | "vertical"] }>();
const tabs = useTabsStore();
const ui = useUiStore();
const root = ref<HTMLElement | null>(null);
type Rect = { x: number; y: number; width: number; height: number };
type Leaf = Rect & { pane: Pane; tabId: string };
type Divider = Rect & { node: SplitNode; tabId: string };
const layout = computed(() => {
  const panes: Leaf[] = [];
  const dividers: Divider[] = [];
  function visit(node: PaneTree, tabId: string, r: Rect) {
    if (isPane(node)) { panes.push({ ...r, pane: node, tabId }); return; }
    dividers.push({ ...r, node, tabId });
    const ratio = Math.max(0.1, Math.min(0.9, node.ratio));
    if (node.direction === "horizontal") {
      visit(node.first, tabId, { ...r, width: r.width * ratio });
      visit(node.second, tabId, { ...r, x: r.x + r.width * ratio, width: r.width * (1 - ratio) });
    } else {
      visit(node.first, tabId, { ...r, height: r.height * ratio });
      visit(node.second, tabId, { ...r, y: r.y + r.height * ratio, height: r.height * (1 - ratio) });
    }
  }
  for (const tab of tabs.tabs) visit(tab.tree, tab.id, { x: 0, y: 0, width: 100, height: 100 });
  return { panes, dividers };
});
function paneStyle(r: Rect): CSSProperties {
  return { left: `${r.x}%`, top: `${r.y}%`, width: `${r.width}%`, height: `${r.height}%` };
}
function dividerStyle(d: Divider): CSSProperties {
  return d.node.direction === "horizontal"
    ? { left: `calc(${d.x + d.width * d.node.ratio}% - 3px)`, top: `${d.y}%`, width: "6px", height: `${d.height}%`, cursor: "col-resize" }
    : { left: `${d.x}%`, top: `calc(${d.y + d.height * d.node.ratio}% - 3px)`, width: `${d.width}%`, height: "6px", cursor: "row-resize" };
}
let stopDrag: (() => void) | undefined;
function resize(event: PointerEvent, d: Divider) {
  if (!root.value || event.button !== 0) return;
  event.preventDefault();
  stopDrag?.();
  const bounds = root.value.getBoundingClientRect();
  const horizontal = d.node.direction === "horizontal";
  const size = (horizontal ? bounds.width * d.width : bounds.height * d.height) / 100;
  if (size <= 0) return;
  const origin = horizontal ? event.clientX : event.clientY;
  const ratio = d.node.ratio;
  const move = (e: PointerEvent) => tabs.setRatio(d.node.id,
    ratio + ((horizontal ? e.clientX : e.clientY) - origin) / size);
  const cleanup = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", cleanup);
    window.removeEventListener("pointercancel", cleanup);
    window.removeEventListener("blur", cleanup);
    stopDrag = undefined;
  };
  stopDrag = cleanup;
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", cleanup);
  window.addEventListener("pointercancel", cleanup);
  window.addEventListener("blur", cleanup);
}
function resizeKey(event: KeyboardEvent, d: Divider) {
  const keys = d.node.direction === "horizontal" ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
  if (event.key === "Home") { event.preventDefault(); tabs.setRatio(d.node.id, 0.5); }
  if (!keys.includes(event.key)) return;
  event.preventDefault();
  event.stopPropagation();
  tabs.setRatio(d.node.id, d.node.ratio + (event.key === keys[0] ? -0.05 : 0.05));
}
// xterm consumes Escape before it reaches the app's bubbling shortcut listener.
// Capture only terminal input here; search fields retain their own Escape behavior.
function fullscreenEscape(event: KeyboardEvent) {
  if (event.key !== "Escape" || !props.visible || !ui.fullscreenPaneId) return;
  if (!(event.target instanceof Element) || !root.value?.contains(event.target) || !event.target.closest(".xterm")) return;
  event.preventDefault();
  event.stopPropagation();
  ui.exitFullscreen();
}
onMounted(() => window.addEventListener("keydown", fullscreenEscape, true));
// Compare individual primitive values so a ratio update does not cancel its drag.
watch([
  () => tabs.activeTabId,
  () => props.visible,
  () => layout.value.panes.map(p => p.pane.id).join(","),
], () => {
  stopDrag?.();
  if (!layout.value.panes.some(p => p.pane.id === ui.fullscreenPaneId && p.tabId === tabs.activeTabId) || !props.visible) ui.exitFullscreen();
});
onBeforeUnmount(() => {
  stopDrag?.();
  window.removeEventListener("keydown", fullscreenEscape, true);
});
</script>

<template>
  <div ref="root" class="relative flex-1 min-h-0" style="background: var(--terminal-border)" data-testid="terminal-workspace">
    <!-- One keyed sibling list for ALL tabs. Never nest terminal owners under the
         split tree: changing tree depth or moving to a tab must not remount xterm. -->
    <div v-for="item in layout.panes" :key="item.pane.id"
      v-show="item.tabId === tabs.activeTabId"
      class="absolute p-px min-w-0 overflow-hidden" :style="paneStyle(item)" :data-pane-id="item.pane.id">
      <TerminalPane :pane="item.pane" :tab-id="item.tabId"
        :visible="visible && item.tabId === tabs.activeTabId"
        @split-h="emit('request-session', item.pane.id, 'horizontal')"
        @split-v="emit('request-session', item.pane.id, 'vertical')"
        @close="tabs.closePane(item.pane.id)" />
    </div>
    <div v-for="d in layout.dividers" :key="d.node.id"
      v-show="d.tabId === tabs.activeTabId && !ui.fullscreenPaneId"
      class="absolute z-20 touch-none hover:bg-primary/40 focus-visible:bg-primary/50 focus-visible:outline-none"
      :style="dividerStyle(d)" role="separator" tabindex="0" aria-label="Resize terminal panes"
      :aria-orientation="d.node.direction === 'horizontal' ? 'vertical' : 'horizontal'"
      :aria-valuenow="Math.round(d.node.ratio * 100)" :aria-valuemin="10" :aria-valuemax="90"
      @pointerdown="resize($event, d)" @keydown="resizeKey($event, d)"
      @dblclick="tabs.setRatio(d.node.id, 0.5)" />
  </div>
</template>
