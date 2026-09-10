<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { useTabsStore, collectPanes } from "../stores/tabs";
import { useUiStore } from "../stores/ui";
import { Plus, X, TerminalSquare, Columns2 } from "lucide-vue-next";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import SessionPicker, { type SessionPlacement } from "./SessionPicker.vue";
import { setDragImageChip } from "../lib/dragChip";

const props = withDefaults(defineProps<{ visible?: boolean }>(), { visible: true });
const emit = defineEmits<{ activate: [] }>();
const tabs = useTabsStore();
const ui = useUiStore();
const picker = ref<InstanceType<typeof SessionPicker> | null>(null);
// Which edge of the hovered tab a tab drag will insert at — the strip shows an
// insertion bar on that side, like reordering tabs in Termius or a browser.
const tabDropSide = ref<"before" | "after" | null>(null);
function activate(id: string) { tabs.setActiveTab(id); emit("activate"); }
function requestSession(paneId: string, direction: SessionPlacement) {
  ui.exitFullscreen();
  tabs.setActivePane(paneId);
  emit("activate");
  picker.value?.show(direction, paneId);
}
function connected(id: string) {
  const tab = tabs.tabs.find(t => t.id === id);
  return tab ? collectPanes(tab.tree).filter(pane => pane.connected).length : 0;
}
function tabLabel(id: string) {
  const tab = tabs.tabs.find(t => t.id === id);
  if (!tab) return "Terminal";
  const panes = collectPanes(tab.tree);
  return panes.length === 1 ? panes[0].title : panes.map(pane => pane.title).join(" + ");
}
function tabDrag(event: DragEvent, id: string) {
  tabs.startTabDrag(id);
  setDragImageChip(event, tabLabel(id));
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", id);
  }
}
function tabDragOver(event: DragEvent, id: string) {
  if (!tabs.draggedPaneId && !tabs.draggedTabId) return;
  if (tabs.draggedTabId === id) return;
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
  tabs.setDragOverTab(id);
  if (tabs.draggedTabId) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    tabDropSide.value = event.clientX < bounds.left + bounds.width / 2 ? "before" : "after";
  }
}
function tabDragLeave(id: string) {
  if (tabs.dragOverTabId === id) { tabs.clearDragOverTab(); tabDropSide.value = null; }
}
function tabDrop(event: DragEvent, target: string) {
  event.preventDefault();
  if (tabs.draggedPaneId) {
    tabs.movePaneToTab(tabs.draggedPaneId, target);
    tabs.endDrag();
    emit("activate");
  } else if (tabs.draggedTabId && tabs.draggedTabId !== target) {
    const from = tabs.tabs.findIndex(t => t.id === tabs.draggedTabId);
    let to = tabs.tabs.findIndex(t => t.id === target) + (tabDropSide.value === "after" ? 1 : 0);
    // The removal of the dragged tab shifts later indexes down by one.
    if (from >= 0 && from < to) to -= 1;
    tabs.reorderTab(from, to);
  }
  tabDropSide.value = null;
}
// Dropping a dragged pane on the empty strip / New-session area extracts it
// into its own tab — the inverse of dropping onto an existing tab's split.
function stripDragOver(event: DragEvent) {
  if (!tabs.draggedPaneId) return;
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
  // Dragover bubbles up from the tab buttons; keep the per-tab highlight and
  // only clear it when hovering strip chrome outside any tab.
  if (!(event.target instanceof HTMLElement) || !event.target.closest(".session-tab")) tabs.clearDragOverTab();
}
function stripDrop(event: DragEvent) {
  if (!tabs.draggedPaneId) return;
  event.preventDefault();
  tabs.extractPaneToNewTab(tabs.draggedPaneId);
  emit("activate");
}
function focusTab(event: KeyboardEvent, index: number) {
  let target: number;
  if (event.key === "ArrowRight") target = (index + 1) % tabs.tabs.length;
  else if (event.key === "ArrowLeft") target = (index - 1 + tabs.tabs.length) % tabs.tabs.length;
  else if (event.key === "Home") target = 0;
  else if (event.key === "End") target = tabs.tabs.length - 1;
  else return;
  event.preventDefault();
  event.stopPropagation();
  const group = (event.currentTarget as HTMLElement).closest("nav");
  group?.querySelectorAll<HTMLButtonElement>("[data-tab-id]")[target]?.focus();
}
function onKeyDown(event: KeyboardEvent) {
  if (!props.visible || document.querySelector("dialog[open]") || event.defaultPrevented) return;
  // Do not steal editing shortcuts from Files, search fields, menus or forms.
  const target = event.target;
  if (target instanceof Element && (target.closest('[role="menu"]') || (target.matches("input, textarea, select") && !target.closest(".xterm")))) return;
  if ((event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey) {
    const directions = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" } as const;
    const direction = directions[event.key as keyof typeof directions];
    if (direction) { event.preventDefault(); tabs.navigatePane(direction); }
  }
}
onMounted(() => window.addEventListener("keydown", onKeyDown));
onUnmounted(() => window.removeEventListener("keydown", onKeyDown));
</script>

<template>
  <section class="flex min-h-0 min-w-0 flex-col" :class="visible ? 'flex-1' : 'shrink-0'" aria-label="Terminal sessions">
    <!-- One navigation row. Session creation lives here; pane actions live in each pane's menu. -->
    <div class="session-strip" data-testid="session-strip"
      @dragover="tabs.draggedPaneId ? stripDragOver($event) : undefined"
      @drop="stripDrop($event)" @dragleave="tabs.draggedPaneId ? tabs.clearDragOverTab() : undefined">
      <nav class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto" aria-label="Open terminal tabs"
        :class="{ 'pane-drag-active': !!tabs.draggedPaneId || !!tabs.draggedTabId }">
        <div v-for="(tab, index) in tabs.tabs" :key="tab.id" class="session-tab"
          :class="{
            'session-tab-active': visible && tab.id === tabs.activeTabId,
            'session-tab-drop-target': tabs.dragOverTabId === tab.id && tabs.draggedPaneId,
            'session-tab-dragging': tabs.draggedTabId === tab.id,
            'session-tab-reorder-before': tabs.draggedTabId && tabs.dragOverTabId === tab.id && tabDropSide === 'before',
            'session-tab-reorder-after': tabs.draggedTabId && tabs.dragOverTabId === tab.id && tabDropSide === 'after',
          }"
          draggable="true" @dragstart="tabDrag($event, tab.id)" @dragend="tabDropSide = null; tabs.endDrag()"
          @dragover="tabDragOver($event, tab.id)" @dragleave="tabDragLeave(tab.id)"
          @drop="tabDrop($event, tab.id)" @auxclick.middle.prevent="tabs.closeTab(tab.id)">
          <button class="session-tab-select" :aria-pressed="visible && tab.id === tabs.activeTabId" :data-tab-id="tab.id"
            :title="`Show ${tab.title} terminal · ${connected(tab.id)} connected`"
            @click="activate(tab.id)" @keydown="focusTab($event, index)">
            <Columns2 v-if="collectPanes(tab.tree).length > 1" class="size-3.5 shrink-0 text-muted-foreground" />
            <TerminalSquare v-else class="size-3.5 shrink-0 text-muted-foreground" />
            <span class="max-w-[220px] truncate">{{ tabLabel(tab.id) }}</span>
            <span class="size-1.5 shrink-0 rounded-full" :class="connected(tab.id) ? 'bg-emerald-500' : 'bg-muted-foreground'" aria-hidden="true" />
          </button>
          <button class="session-tab-close" :aria-label="`Close tab ${tab.title}`" @click="tabs.closeTab(tab.id)"><X class="size-3.5" /></button>
        </div>
        <span v-if="!tabs.tabs.length" class="px-2 text-xs text-muted-foreground">No open sessions</span>
      </nav>
      <button class="new-session" :class="{ 'new-session-drop-target': !!tabs.draggedPaneId }"
        :aria-label="tabs.draggedPaneId ? 'Drop here for a new tab' : 'New session'"
        :title="tabs.draggedPaneId ? 'Drop to move pane into its own tab' : 'New session: host or local shell'"
        @click="picker?.show()">
        <Plus class="size-4" :stroke-width="1.75" /><span class="hidden sm:inline">{{ tabs.draggedPaneId ? 'New tab' : 'New session' }}</span>
      </button>
    </div>
    <TerminalWorkspace v-show="visible && tabs.tabs.length > 0" :visible="visible" @request-session="requestSession" />
    <div v-if="visible && tabs.tabs.length === 0" class="terminal-empty">
      <TerminalSquare class="size-12 text-muted-foreground/50" :stroke-width="1.25" />
      <h1 class="text-lg font-medium text-foreground">Your next session starts here.</h1>
      <p class="max-w-sm text-sm leading-6 text-muted-foreground">Use New session to connect a host or open a local shell. Keep them in tabs or work side by side.</p>
    </div>
    <SessionPicker ref="picker" @opened="emit('activate')" />
  </section>
</template>

<style scoped>
.session-strip { @apply flex h-12 shrink-0 items-center gap-2 px-3 pl-12 md:pl-3 border-b border-sidebar-border; background: var(--workspace-chrome); color: hsl(var(--sidebar-foreground)); }
/* Drop affordances ease in/out: a resting zero-width inset shadow lets the
   drop-target ring interpolate instead of snapping on. */
.session-tab { @apply relative flex h-9 shrink-0 items-center rounded-md; background: hsl(var(--muted)); box-shadow: inset 0 0 0 0 transparent; transition: opacity 140ms ease-out, box-shadow 160ms ease-out, background-color 120ms ease-out; }
.session-tab-active { background: hsl(var(--accent)); box-shadow: inset 0 -2px var(--workspace-accent); }
.session-tab-select { @apply flex h-9 min-w-0 items-center gap-2 rounded-md px-3 text-[12px] text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring; }
.session-tab-active .session-tab-select { @apply text-foreground; }
.session-tab-close { @apply mr-1 flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-background hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring; }
.new-session { @apply ml-1 flex h-8 shrink-0 items-center justify-center gap-1.5 rounded-md px-2.5 text-xs font-medium text-foreground hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring; box-shadow: inset 0 0 0 0 transparent; transition: box-shadow 160ms ease-out, background-color 120ms ease-out; }
.terminal-empty { @apply flex flex-1 flex-col items-center justify-center gap-4 p-8 text-center; background: var(--terminal-background); }
/* Pane-drag affordances in the tab strip, mirroring Termius workspaces. */
.session-tab-drop-target { background: hsl(var(--primary) / 0.15); box-shadow: inset 0 0 0 2px hsl(var(--primary)); }
.new-session-drop-target { background: hsl(var(--primary) / 0.15); box-shadow: inset 0 0 0 2px hsl(var(--primary)); }
/* Tab-drag affordances: the dragged tab dims and the hovered neighbour shows
   an insertion caret on the side the tab will land on. */
.session-tab-dragging { opacity: 0.45; }
.session-tab-reorder-before::before, .session-tab-reorder-after::after {
  content: ""; position: absolute; top: 3px; bottom: 3px; width: 2.5px;
  border-radius: 9999px; background: var(--workspace-accent);
  pointer-events: none;
  animation: session-insert-in 150ms cubic-bezier(0.23, 1, 0.32, 1);
}
.session-tab-reorder-before::before { left: 1.5px; }
.session-tab-reorder-after::after { right: 1.5px; }
@keyframes session-insert-in { from { transform: scaleY(0.2); opacity: 0; } }
</style>
