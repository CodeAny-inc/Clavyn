<script setup lang="ts">
import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";
import { useTabsStore, type Pane, type DropPosition } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { useVaultStore } from "../stores/vault";
import { useUiStore } from "../stores/ui";
import ActionMenu, { type MenuAction } from "./ui/ActionMenu.vue";
import * as api from "../api";
import { SplitSquareHorizontal, SplitSquareVertical, X, GripVertical, Maximize2, Minimize2,
  RotateCw, Search, ChevronUp, ChevronDown, CaseSensitive, Regex, WholeWord, Loader2, ArrowUpRight } from "lucide-vue-next";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { Host } from "../types";

const props = withDefaults(defineProps<{ pane: Pane; tabId: string; visible?: boolean }>(), { visible: true });
const emit = defineEmits<{ "split-h": []; "split-v": []; close: [] }>();
const tabs = useTabsStore();
const hosts = useHostsStore();
const identities = useIdentitiesStore();
const vault = useVaultStore();
const ui = useUiStore();
const containerRef = ref<HTMLElement | null>(null);
const paneRef = ref<HTMLElement | null>(null);
const connecting = ref(false);
const error = ref("");
const showSearch = ref(false);
const searchQuery = ref("");
const searchCaseSensitive = ref(false);
const searchRegex = ref(false);
const searchWholeWord = ref(false);
const searchInputRef = ref<HTMLInputElement | null>(null);
const connectedEndpoint = ref<string | null>(props.pane.terminalType === "local" ? "Local shell" : null);
let term: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let searchAddon: SearchAddon | null = null;
let observer: ResizeObserver | null = null;
const listeners: UnlistenFn[] = [];
let currentSessionId: string | null = null;
let disposed = false;
let sessionEnded = false;
const listenersReady = ref(false);
const isActive = computed(() => props.visible && tabs.activeTabId === props.tabId && tabs.activePaneId === props.pane.id);
const isFullscreen = computed(() => ui.fullscreenPaneId === props.pane.id);
const isDragging = computed(() => tabs.draggedPaneId === props.pane.id);
const isDragOver = computed(() => tabs.dragOverPaneId === props.pane.id);
const status = computed(() => connecting.value ? "Connecting" : props.pane.connected ? "Connected" : "Disconnected");
const otherTabs = computed(() => tabs.tabs.filter(t => t.id !== props.tabId));

function formatHostEndpoint(host: Host) {
  // Match the backend: a resolved identity overrides the saved host username.
  const identity = identities.identities.find(item => item.id === host.identity_id);
  return `${identity?.username ?? host.username}@${host.hostname}:${host.port}`;
}
const configuredEndpoint = computed(() => {
  if (props.pane.terminalType === "local") return "Local shell";
  const host = hosts.hosts.find(item => item.id === props.pane.hostId);
  return host ? formatHostEndpoint(host) : props.pane.title;
});
const hostAddress = computed(() => connectedEndpoint.value ?? configuredEndpoint.value);

function focusInput() {
  if (disposed || !isActive.value) return;
  // An open search belongs to this pane too. Never leave focus on another host.
  if (showSearch.value) searchInputRef.value?.focus();
  else term?.focus();
}
function fit(focus = false) {
  if (disposed || !props.visible || !term || !fitAddon) return;
  try {
    if (containerRef.value?.clientWidth && containerRef.value.clientHeight) {
      fitAddon.fit();
      if (currentSessionId && props.pane.connected) void api.sessionResize(currentSessionId, term.cols, term.rows).catch(() => {});
    }
  } catch { /* ResizeObserver can run during teardown. */ }
  // Focus restoration must not depend on the resize measurement being ready.
  if (focus) focusInput();
}
// Exit before the destination pane's post-render watcher can focus its input.
// This also covers same-tab shortcuts, not only changes to the active tab/tree.
watch(isActive, active => {
  if (!active && isFullscreen.value) ui.exitFullscreen();
}, { flush: "sync" });
watch([isActive, () => props.visible], () => nextTick(() => {
  // Focusing an inactive pane's controls activates it too. Do not steal focus
  // back from that control or its teleported menu before the user can use it.
  const focused = document.activeElement;
  const controlHasFocus = focused instanceof Element &&
    paneRef.value?.contains(focused) && !focused.closest(".xterm");
  const menuId = paneRef.value?.querySelector('[aria-haspopup="menu"]')?.getAttribute("aria-controls");
  const menuHasFocus = !!menuId && document.getElementById(menuId)?.contains(focused);
  fit(!controlHasFocus && !menuHasFocus);
}), { flush: "post" });
// Explicit fullscreen changes still return input to this pane's terminal/search.
watch(isFullscreen, () => nextTick(() => fit(true)), { flush: "post" });
function activatePane(focusTerminal = false) {
  tabs.setActivePane(props.pane.id);
  if (focusTerminal) focusInput();
}
function focusPane() { activatePane(true); }
function writeError(message: string) {
  error.value = message;
  term?.write(`\r\n\x1b[31m${message}\x1b[0m\r\n`);
}
async function connectSession() {
  if (disposed || connecting.value || !term || !listenersReady.value) return;
  connecting.value = true;
  error.value = "";
  tabs.setPaneDisconnected(props.pane.id);
  const previous = currentSessionId;
  const sessionId = crypto.randomUUID();
  currentSessionId = sessionId;
  sessionEnded = false;
  let endpointForAttempt = "Local shell";
  try {
    if (previous) await api.closeSession(previous).catch(() => {});
    if (disposed) return;
    const terminal = term;
    if (!terminal) return;
    if (previous) {
      // Old-session events are already filtered by currentSessionId. Drain any
      // output queued in xterm before resetting, so it cannot re-enable old modes.
      await new Promise<void>(resolve => terminal.write("", resolve));
      if (disposed) return;
      searchAddon?.clearDecorations();
      terminal.reset();
    }
    fit();
    if (props.pane.terminalType === "local") {
      await api.createLocalTerminal(sessionId, terminal.cols, terminal.rows);
    } else {
      const host = hosts.hosts.find(h => h.id === props.pane.hostId);
      if (!host) throw new Error("Host not found. Check the saved host configuration.");
      const identity = identities.identities.find(i => i.id === host.identity_id);
      const auth = identity?.auth ?? host.auth;
      if (auth === "publickey" && !vault.unlocked) {
        if (!await ui.requestVaultUnlock()) throw new Error("Connection cancelled: vault remains locked.");
        if (disposed) return;
      }
      const openSsh = () => {
        // Resolve after any unlock wait, then freeze the endpoint for this attempt.
        endpointForAttempt = formatHostEndpoint(host);
        return api.connectSsh(sessionId, host, null, terminal.cols, terminal.rows);
      };
      try {
        await openSsh();
      } catch (cause) {
        const message = String(cause);
        if (!message.includes("vault passphrase required") && !message.includes("vault required")) throw cause;
        if (!await ui.requestVaultUnlock()) throw new Error("Connection cancelled: vault remains locked.");
        if (disposed) return;
        await openSsh();
      }
    }
    // Closing while connect is pending must not resurrect a session or leak it.
    if (disposed || props.pane.closing) {
      await api.closeSession(sessionId).catch(() => {});
      return;
    }
    if (!sessionEnded) {
      // Keep the UI tied to the endpoint that actually owns this successful session.
      // Editing/deleting the saved Host must not relabel a live shell.
      connectedEndpoint.value = endpointForAttempt;
      tabs.setPaneConnected(props.pane.id, sessionId);
    }
    fit(true);
  } catch (cause) {
    if (!disposed) writeError(`Connection failed: ${String(cause)}`);
  } finally {
    connecting.value = false;
  }
}

onMounted(async () => {
  if (!containerRef.value) return;
  term = new Terminal({ fontSize: 13,
    fontFamily: "'SFMono-Regular', 'SF Mono', 'Cascadia Code', 'Roboto Mono', ui-monospace, monospace",
    theme: { background: getComputedStyle(document.documentElement).getPropertyValue("--terminal-background").trim() || "#10151e", foreground: "#e6e9ef", cursor: "#4f9cf9", selectionBackground: "#264f78" },
    cursorBlink: true, scrollback: 10000 });
  fitAddon = new FitAddon();
  searchAddon = new SearchAddon();
  term.loadAddon(fitAddon);
  term.loadAddon(searchAddon);
  term.attachCustomKeyEventHandler(event => {
    const command = event.metaKey || event.ctrlKey;
    if (command && event.key.toLowerCase() === "f") {
      if (event.type === "keydown") { event.preventDefault(); openSearch(); }
      return false;
    }
    const directions = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" } as const;
    const direction = directions[event.key as keyof typeof directions];
    if (command && !event.shiftKey && !event.altKey && direction) {
      if (event.type === "keydown") { event.preventDefault(); tabs.navigatePane(direction); }
      return false;
    }
    return true;
  });
  term.open(containerRef.value);
  term.onData(data => {
    if (currentSessionId && props.pane.connected && !connecting.value)
      void api.sessionWrite(currentSessionId, Array.from(new TextEncoder().encode(data))).catch(cause => {
        if (!disposed) writeError(`Write failed: ${String(cause)}`);
      });
  });
  observer = new ResizeObserver(() => fit());
  observer.observe(containerRef.value);
  fit();
  try {
    const dataListener = await api.onSessionData(event => {
      if (!disposed && event.session_id === currentSessionId) term?.write(new Uint8Array(event.data));
    });
    if (disposed) { dataListener(); return; }
    listeners.push(dataListener);
    const closeListener = await api.onSessionClosed(event => {
      if (!disposed && event.session_id === currentSessionId) {
        sessionEnded = true;
        tabs.setPaneDisconnected(props.pane.id);
        writeError(`Session closed: ${event.reason}`);
      }
    });
    if (disposed) { closeListener(); return; }
    listeners.push(closeListener);
    listenersReady.value = true;
    await connectSession();
  } catch (cause) {
    if (!disposed) writeError(`Could not initialize terminal: ${String(cause)}`);
  }
});
onBeforeUnmount(() => {
  disposed = true;
  listeners.forEach(unlisten => unlisten());
  observer?.disconnect();
  term?.dispose();
  term = null;
  // Explicit close in the store already owns an established session's teardown.
  if (currentSessionId && (!props.pane.closing || props.pane.sessionId !== currentSessionId))
    void api.closeSession(currentSessionId).catch(() => {});
});
function openSearch() {
  activatePane();
  showSearch.value = true;
  nextTick(() => {
    if (disposed || !isActive.value) return;
    focusInput();
    searchInputRef.value?.select();
  });
}
function closeSearch() {
  activatePane();
  showSearch.value = false;
  searchAddon?.clearDecorations();
  focusInput();
}
function toggleFullscreen() {
  activatePane();
  ui.toggleFullscreen(props.pane.id);
}
function doSearch(previous = false) {
  if (!searchQuery.value) { searchAddon?.clearDecorations(); return; }
  const options = { caseSensitive: searchCaseSensitive.value, regex: searchRegex.value, wholeWord: searchWholeWord.value,
    decorations: { matchOverviewRuler: "#4f9cf9", activeMatchColorOverviewRuler: "#f59e0b", matchBackground: "#264f78", activeMatchBackground: "#f59e0b80" } };
  try {
    if (previous) searchAddon?.findPrevious(searchQuery.value, options);
    else searchAddon?.findNext(searchQuery.value, options);
  } catch { /* An incomplete regular expression should not break the terminal. */ }
}
function searchKey(event: KeyboardEvent) {
  if (event.key === "Escape") { event.preventDefault(); closeSearch(); }
  else if (event.key === "Enter" || ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "g")) {
    event.preventDefault(); doSearch(event.shiftKey);
  }
}
function startDrag(event: DragEvent) {
  tabs.startDrag(props.pane.id);
  if (event.dataTransfer) { event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", props.pane.id); }
}
function dropPosition(event: DragEvent): DropPosition {
  const bounds = paneRef.value!.getBoundingClientRect();
  const x = (event.clientX - bounds.left) / bounds.width;
  const y = (event.clientY - bounds.top) / bounds.height;
  if (x > 0.3 && x < 0.7 && y > 0.3 && y < 0.7) return "center";
  const edges: [DropPosition, number][] = [["left", x], ["right", 1 - x], ["top", y], ["bottom", 1 - y]];
  return edges.sort((a, b) => a[1] - b[1])[0][0];
}
function dragOver(event: DragEvent) {
  if (!tabs.draggedPaneId || isDragging.value || !paneRef.value) return;
  event.preventDefault();
  tabs.setDragOver(props.pane.id, dropPosition(event));
}
function drop(event: DragEvent) {
  if (!tabs.draggedPaneId || !paneRef.value) return;
  event.preventDefault();
  tabs.dropPane(props.pane.id, dropPosition(event));
}
const actions = computed<MenuAction[]>(() => [
  { id: "split-h", label: "Split right…", icon: SplitSquareHorizontal },
  { id: "split-v", label: "Split below…", icon: SplitSquareVertical },
  { id: "search", label: "Find in terminal", icon: Search, shortcut: "⌘ / Ctrl F", separator: true },
  { id: "fullscreen", label: isFullscreen.value ? "Restore pane" : "Maximize pane", icon: isFullscreen.value ? Minimize2 : Maximize2 },
  ...otherTabs.value.map((tab, index) => ({ id: `move:${tab.id}`, label: `Move to ${tab.title}`, icon: ArrowUpRight, separator: index === 0 })),
  ...(!props.pane.connected ? [{ id: "reconnect", label: "Reconnect", icon: RotateCw, disabled: connecting.value || !listenersReady.value, separator: true }] : []),
  { id: "close", label: "Close session", icon: X, danger: true, separator: true },
]);
function selectAction(id: string) {
  // ActionMenu is teleported to <body>, so it cannot rely on the pane's bubbling click handler.
  activatePane();
  if (id === "split-h") emit("split-h");
  else if (id === "split-v") emit("split-v");
  else if (id === "search") openSearch();
  else if (id === "fullscreen") toggleFullscreen();
  else if (id === "reconnect") void connectSession();
  else if (id === "close") emit("close");
  else if (id.startsWith("move:")) tabs.movePaneToTab(props.pane.id, id.slice(5));
}
</script>

<template>
  <div ref="paneRef" class="terminal-pane flex h-full w-full min-w-0 flex-col"
    :class="[isFullscreen ? 'fixed inset-0 z-[90]' : 'relative', isActive ? 'ring-1 ring-inset ring-blue-400/50' : '']"
    :data-session-id="pane.sessionId" :data-connected="pane.connected" :data-host-id="pane.hostId" :data-active="isActive"
    @click="focusPane" @dragover="dragOver" @drop="drop"
    @dragleave="!paneRef?.contains($event.relatedTarget as Node) && tabs.clearDragOver()">
    <header class="pane-header" :class="{ 'pane-header-active': isActive }" data-testid="pane-header">
      <div class="flex min-w-0 flex-1 cursor-grab items-center gap-2" draggable="true"
        :aria-label="`Drag pane ${pane.title}`" @dragstart="startDrag" @dragend="tabs.endDrag()">
        <GripVertical class="size-3 shrink-0 text-slate-500" />
        <Loader2 v-if="connecting" class="size-3 shrink-0 animate-spin text-blue-300" aria-label="Connecting" />
        <span v-else class="size-1.5 shrink-0 rounded-full" :class="pane.connected ? 'bg-emerald-400' : 'bg-slate-500'" :title="status" />
        <span class="truncate text-[12px]" :title="`${pane.title} · ${hostAddress} · ${status}`">{{ hostAddress }}</span>
        <span class="sr-only" role="status">{{ status }}</span>
      </div>
      <div class="flex shrink-0 items-center gap-0.5 text-slate-400" @click.stop
        @pointerdown.capture="activatePane()" @focusin="activatePane()">
        <button class="pane-button" aria-label="Search in terminal" title="Search (Cmd/Ctrl+F)" @click="openSearch"><Search class="size-3.5" /></button>
        <button class="pane-button" :aria-label="isFullscreen ? 'Exit fullscreen' : 'Fullscreen'" :title="isFullscreen ? 'Restore pane (Escape)' : 'Maximize pane'" @click="toggleFullscreen"><Minimize2 v-if="isFullscreen" class="size-3.5" /><Maximize2 v-else class="size-3.5" /></button>
        <ActionMenu :label="`Actions for ${pane.title}`" :items="actions" :enabled="visible" @select="selectAction" />
      </div>
    </header>
    <div v-if="error" class="flex shrink-0 items-center gap-2 border-b border-border bg-background px-3 py-2 text-xs" role="alert">
      <span class="min-w-0 flex-1 text-muted-foreground">{{ error }}</span>
      <button v-if="!pane.connected" class="shrink-0 text-primary disabled:opacity-50" :disabled="connecting" @click.stop="activatePane(); connectSession()">Reconnect</button>
    </div>
    <div ref="containerRef" class="min-h-0 flex-1 overflow-hidden" />
    <div v-if="showSearch" class="absolute right-2 top-10 z-40 flex max-w-[calc(100%-16px)] flex-wrap items-center gap-1 rounded-md border border-border bg-background p-1 shadow-lg" @click.stop @keydown.stop="searchKey" @focusin="activatePane()">
      <input ref="searchInputRef" v-model="searchQuery" aria-label="Search terminal output" placeholder="Search..." class="h-7 w-36 min-w-0 bg-transparent px-2 text-xs outline-none" @input="doSearch()" />
      <button class="pane-button" :aria-pressed="searchCaseSensitive" aria-label="Case sensitive" @click="searchCaseSensitive = !searchCaseSensitive; doSearch()"><CaseSensitive class="size-3.5" /></button>
      <button class="pane-button" :aria-pressed="searchWholeWord" aria-label="Whole word" @click="searchWholeWord = !searchWholeWord; doSearch()"><WholeWord class="size-3.5" /></button>
      <button class="pane-button" :aria-pressed="searchRegex" aria-label="Regular expression" @click="searchRegex = !searchRegex; doSearch()"><Regex class="size-3.5" /></button>
      <button class="pane-button" aria-label="Previous match" @click="doSearch(true)"><ChevronUp class="size-3.5" /></button>
      <button class="pane-button" aria-label="Next match" @click="doSearch()"><ChevronDown class="size-3.5" /></button>
      <button class="pane-button" aria-label="Close search" @click="closeSearch"><X class="size-3.5" /></button>
    </div>
    <div v-if="isDragOver && tabs.draggedPaneId && !isDragging" class="pointer-events-none absolute inset-0 z-30 flex items-center justify-center border-2 border-dashed border-primary bg-primary/20">
      <span class="rounded bg-primary px-3 py-2 text-xs text-primary-foreground">{{ tabs.dragOverPosition === 'center' ? 'Swap pane positions' : `Move pane to ${tabs.dragOverPosition}` }}</span>
    </div>
  </div>
</template>

<style scoped>
.terminal-pane { background: var(--terminal-background); }
.pane-header { @apply flex h-9 shrink-0 items-center gap-2 border-b px-2 text-slate-400; background: var(--terminal-toolbar); border-color: var(--terminal-border); }
.pane-header-active { background: var(--terminal-toolbar-active); color: #e3e8f1; box-shadow: inset 2px 0 var(--workspace-accent); }
.pane-button { @apply flex size-8 items-center justify-center rounded-md text-current hover:bg-white/10 disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring; }
.pane-button[aria-pressed="true"] { @apply bg-primary/20 text-primary; }
</style>
