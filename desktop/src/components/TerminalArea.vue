<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { useTabsStore, collectPanes } from "../stores/tabs";
import { useUiStore } from "../stores/ui";
import { Plus, X, TerminalSquare, Columns2 } from "lucide-vue-next";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import SessionPicker, { type SessionPlacement } from "./SessionPicker.vue";

const props = withDefaults(defineProps<{ visible?: boolean }>(), { visible: true });
const emit = defineEmits<{ activate: [] }>();
const tabs = useTabsStore();
const ui = useUiStore();
const picker = ref<InstanceType<typeof SessionPicker> | null>(null);
const draggedTabId = ref<string | null>(null);
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
  draggedTabId.value = id;
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", id);
  }
}
function tabDrop(event: DragEvent, target: string) {
  event.preventDefault();
  if (tabs.draggedPaneId) {
    tabs.movePaneToTab(tabs.draggedPaneId, target);
    tabs.endDrag();
    emit("activate");
  } else if (draggedTabId.value) {
    tabs.reorderTab(tabs.tabs.findIndex(t => t.id === draggedTabId.value), tabs.tabs.findIndex(t => t.id === target));
  }
  draggedTabId.value = null;
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
    <div v-show="!ui.fullscreenPaneId" class="session-strip" data-testid="session-strip">
      <nav class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto" aria-label="Open terminal tabs">
        <div v-for="(tab, index) in tabs.tabs" :key="tab.id" class="session-tab"
          :class="{ 'session-tab-active': visible && tab.id === tabs.activeTabId }"
          draggable="true" @dragstart="tabDrag($event, tab.id)" @dragend="draggedTabId = null"
          @dragover.prevent @drop="tabDrop($event, tab.id)" @auxclick.middle.prevent="tabs.closeTab(tab.id)">
          <button class="session-tab-select" :aria-pressed="visible && tab.id === tabs.activeTabId" :data-tab-id="tab.id"
            :title="`Show ${tab.title} terminal · ${connected(tab.id)} connected`"
            @click="activate(tab.id)" @keydown="focusTab($event, index)">
            <Columns2 v-if="collectPanes(tab.tree).length > 1" class="size-3.5 shrink-0 text-blue-300" />
            <TerminalSquare v-else class="size-3.5 shrink-0 text-slate-300" />
            <span class="max-w-[220px] truncate">{{ tabLabel(tab.id) }}</span>
            <span class="size-1.5 shrink-0 rounded-full" :class="connected(tab.id) ? 'bg-emerald-400' : 'bg-slate-500'" aria-hidden="true" />
          </button>
          <button class="session-tab-close" :aria-label="`Close tab ${tab.title}`" @click="tabs.closeTab(tab.id)"><X class="size-3.5" /></button>
        </div>
        <span v-if="!tabs.tabs.length" class="px-2 text-xs text-slate-400">No open sessions</span>
      </nav>
      <button class="new-session" aria-label="New session" title="New session: host or local shell" @click="picker?.show()">
        <Plus class="size-4" :stroke-width="1.75" /><span class="hidden sm:inline">New session</span>
      </button>
    </div>
    <TerminalWorkspace v-show="visible && tabs.tabs.length > 0" :visible="visible" @request-session="requestSession" />
    <div v-if="visible && tabs.tabs.length === 0" class="terminal-empty">
      <TerminalSquare class="size-12 text-slate-600" :stroke-width="1.25" />
      <h1 class="text-lg font-medium text-slate-200">Your next session starts here.</h1>
      <p class="max-w-sm text-sm leading-6 text-slate-400">Use New session to connect a host or open a local shell. Keep them in tabs or work side by side.</p>
    </div>
    <SessionPicker ref="picker" @opened="emit('activate')" />
  </section>
</template>

<style scoped>
.session-strip { @apply flex h-12 shrink-0 items-center gap-2 px-3 pl-12 md:pl-3; background: var(--workspace-chrome); color: #e3e8f1; }
.session-tab { @apply flex h-9 shrink-0 items-center rounded-lg; background: #33394a; }
.session-tab-active { background: #454e64; box-shadow: inset 0 -2px var(--workspace-accent); }
.session-tab-select { @apply flex h-9 min-w-0 items-center gap-2 rounded-lg px-3 text-[12px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400; }
.session-tab-close { @apply mr-1 flex size-7 items-center justify-center rounded-md text-slate-400 hover:bg-white/10 hover:text-white focus-visible:ring-2 focus-visible:ring-blue-400; }
.new-session { @apply ml-1 flex h-8 shrink-0 items-center justify-center gap-1.5 rounded-lg px-2.5 text-xs font-medium text-slate-200 hover:bg-white/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400; }
.terminal-empty { @apply flex flex-1 flex-col items-center justify-center gap-4 p-8 text-center; background: var(--terminal-background); }
</style>
