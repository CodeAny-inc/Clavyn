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
import SshPasswordPrompt from "./SshPasswordPrompt.vue";
import { useFocusIntent } from "../composables/useFocusIntent";
import { configuredSshEndpoint, effectiveSshIdentity, formatSshEndpoint, passwordAuth, sshConfigurationKey } from "../lib/sshIdentity";
import * as api from "../api";
import { SplitSquareHorizontal, SplitSquareVertical, X, GripVertical, Maximize2, Minimize2,
  RotateCw, Search, ChevronUp, ChevronDown, CaseSensitive, Regex, WholeWord, Loader2, ArrowUpRight } from "lucide-vue-next";
import type { UnlistenFn } from "@tauri-apps/api/event";

const props = withDefaults(defineProps<{ pane: Pane; tabId: string; visible?: boolean }>(), { visible: true });
const emit = defineEmits<{ "split-h": []; "split-v": []; close: [] }>();
const tabs = useTabsStore();
const hosts = useHostsStore();
const identities = useIdentitiesStore();
const vault = useVaultStore();
const ui = useUiStore();
const { capture: captureFocusIntent, blocked: terminalFocusBlocked } = useFocusIntent();
const containerRef = ref<HTMLElement | null>(null);
const paneRef = ref<HTMLElement | null>(null);
const passwordPrompt = ref<InstanceType<typeof SshPasswordPrompt> | null>(null);
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
// Buffer remote output, never user input, until this attempt can answer queries.
let pendingOutput: Uint8Array[] | null = null;
const listenersReady = ref(false);
const isActive = computed(() => props.visible && tabs.activeTabId === props.tabId && tabs.activePaneId === props.pane.id);
const isFullscreen = computed(() => ui.fullscreenPaneId === props.pane.id);
const isDragging = computed(() => tabs.draggedPaneId === props.pane.id);
const isDragOver = computed(() => tabs.dragOverPaneId === props.pane.id);
const status = computed(() => connecting.value ? "Connecting" : props.pane.connected ? "Connected" : "Disconnected");
const otherTabs = computed(() => tabs.tabs.filter(t => t.id !== props.tabId));

const configuredEndpoint = computed(() => {
  if (props.pane.terminalType === "local") return "Local shell";
  const host = hosts.hosts.find(item => item.id === props.pane.hostId);
  if (!host) return props.pane.title;
  if (host.identity_id && !identities.loaded) return `Resolving SSH identity · ${host.hostname}:${host.port}`;
  return configuredSshEndpoint(host, identities.identities);
});
const hostAddress = computed(() => connectedEndpoint.value ?? configuredEndpoint.value);

function focusInput() {
  if (disposed || !isActive.value || terminalFocusBlocked()) return;
  if (passwordPrompt.value?.pending) passwordPrompt.value.focus();
  else if (showSearch.value) searchInputRef.value?.focus();
  else term?.focus();
}
// Geometry updates are never permission to change the user's input destination.
function fit() {
  if (disposed || !props.visible || !term || !fitAddon) return;
  try {
    if (containerRef.value?.clientWidth && containerRef.value.clientHeight) {
      fitAddon.fit();
      if (currentSessionId && props.pane.connected) void api.sessionResize(currentSessionId, term.cols, term.rows).catch(() => {});
    }
  } catch { /* ResizeObserver can run during teardown. */ }
}
function controlHasFocus() {
  const focused = document.activeElement;
  return focused instanceof Element && !!paneRef.value?.contains(focused) && !focused.closest(".xterm");
}
function queueFocus(preserveControls = false, stillCurrent: () => boolean = () => true) {
  const current = captureFocusIntent();
  void nextTick(() => {
    fit();
    if (current() && stillCurrent() && (!preserveControls || !controlHasFocus())) focusInput();
  });
}
watch(isActive, active => {
  if (!active && isFullscreen.value) ui.exitFullscreen();
}, { flush: "sync" });
watch([isActive, () => props.visible], () => queueFocus(true), { flush: "post" });
// A successful move can preserve activePaneId while the DOM move loses focus.
watch(() => tabs.paneFocusRequest, request => {
  if (request?.paneId === props.pane.id) queueFocus(false, () => request === tabs.paneFocusRequest);
}, { flush: "post" });
watch(isFullscreen, () => queueFocus(), { flush: "post" });
watch(() => ui.showVaultUnlockModal, (open, wasOpen) => {
  // Closing the shared prompt returns input to the active waiter, not whichever
  // network connection happens to complete last. Respect any newer overlay.
  if (!open && wasOpen) queueFocus(false, () => document.activeElement === document.body);
}, { flush: "post" });
function activatePane(focusTerminal = false) {
  tabs.setActivePane(props.pane.id);
  if (focusTerminal) focusInput();
}
function focusPane() { activatePane(true); }
function reconnect() {
  activatePane(true);
  void connectSession();
}
function writeError(message: string) {
  error.value = message;
  term?.write(`\r\n\x1b[31m${message}\x1b[0m\r\n`);
}
async function connectSession() {
  if (disposed || connecting.value || !term || !listenersReady.value) return;
  const mayAutofocusPassword = captureFocusIntent();
  connecting.value = true;
  error.value = "";
  tabs.setPaneDisconnected(props.pane.id);
  const previous = currentSessionId;
  const sessionId = crypto.randomUUID();
  currentSessionId = sessionId;
  sessionEnded = false;
  pendingOutput = [];
  let endpointForAttempt = "Local shell";
  try {
    if (previous) await api.closeSession(previous).catch(() => {});
    if (disposed) return;
    const terminal = term;
    if (!terminal) return;
    if (previous) {
      // Old-session events are filtered before draining/resetting the emulator.
      await new Promise<void>(resolve => terminal.write("", resolve));
      if (disposed) return;
      searchAddon?.clearDecorations();
      terminal.reset();
    }
    fit();
    if (props.pane.terminalType === "local") {
      await api.createLocalTerminal(sessionId, terminal.cols, terminal.rows);
    } else {
      // All creation paths, including HostList and restored workspaces, wait for
      // a successful identity load. An empty initial cache is not a missing identity.
      await identities.ensureLoaded();
      if (disposed || props.pane.closing) return;
      const host = hosts.hosts.find(h => h.id === props.pane.hostId);
      if (!host) throw new Error("Host not found. Check the saved host configuration.");
      const auth = effectiveSshIdentity(host, identities.identities).auth;
      if (auth === "publickey" && !vault.unlocked) {
        if (!await ui.requestVaultUnlock()) throw new Error("Connection cancelled: vault remains locked.");
        if (disposed) return;
      }
      const openSsh = async () => {
        await identities.ensureLoaded();
        if (disposed || props.pane.closing) return;
        // Re-read after unlock: configuration may have been edited while waiting.
        const host = hosts.hosts.find(h => h.id === props.pane.hostId);
        if (!host) throw new Error("Host not found. Check the saved host configuration.");
        const effective = effectiveSshIdentity(host, identities.identities);
        const key = sshConfigurationKey(host, identities.identities);
        endpointForAttempt = configuredSshEndpoint(host, identities.identities);
        let password: string | null = null;
        try {
          if (passwordAuth(effective.auth)) {
            if (!passwordPrompt.value) throw new Error("Password prompt is not ready. Reconnect to retry.");
            password = await passwordPrompt.value.request(endpointForAttempt, mayAutofocusPassword);
            if (disposed || props.pane.closing) return;
            if (password === null) throw new Error("Connection cancelled.");
            const latest = hosts.hosts.find(h => h.id === props.pane.hostId);
            if (!identities.loaded || !latest || sshConfigurationKey(latest, identities.identities) !== key)
              throw new Error("Connection settings changed. Reconnect to review the updated account.");
          }
          const request = api.connectSsh(sessionId, host, password, terminal.cols, terminal.rows, effective.username);
          // The IPC request owns its serialized argument; retain no reusable credential.
          password = null;
          const connected = await request;
          // Rust returns metadata from the same identity snapshot it authenticated.
          // The preflight endpoint also supports older bridges returning no metadata.
          if (connected) endpointForAttempt = formatSshEndpoint(connected);
        } finally {
          password = null;
        }
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
    if (disposed || props.pane.closing) {
      await api.closeSession(sessionId).catch(() => {});
      return;
    }
    if (!sessionEnded) {
      connectedEndpoint.value = endpointForAttempt;
      tabs.setPaneConnected(props.pane.id, sessionId);
      // Enable writes before xterm parses queued queries (including synchronous
      // parser callbacks). Keep chunk order and the same session owner.
      connecting.value = false;
      const output = pendingOutput;
      pendingOutput = null;
      for (const chunk of output ?? []) terminal.write(chunk);
    }
    fit();
  } catch (cause) {
    if (!disposed) writeError(`Connection failed: ${String(cause)}`);
  } finally {
    pendingOutput = null;
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
  // Session creation is the focus intent; async connection completion is not.
  focusInput();
  try {
    const dataListener = await api.onSessionData(event => {
      if (disposed || props.pane.closing || sessionEnded || event.session_id !== currentSessionId) return;
      const data = new Uint8Array(event.data);
      if (pendingOutput) pendingOutput.push(data);
      else if (props.pane.connected) term?.write(data);
    });
    if (disposed) { dataListener(); return; }
    listeners.push(dataListener);
    const closeListener = await api.onSessionClosed(event => {
      if (!disposed && event.session_id === currentSessionId) {
        sessionEnded = true;
        pendingOutput = null;
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
  pendingOutput = null;
  passwordPrompt.value?.cancel();
  listeners.forEach(unlisten => unlisten());
  observer?.disconnect();
  term?.dispose();
  term = null;
  if (currentSessionId && (!props.pane.closing || props.pane.sessionId !== currentSessionId))
    void api.closeSession(currentSessionId).catch(() => {});
});
function openSearch() {
  activatePane();
  showSearch.value = true;
  const current = captureFocusIntent();
  nextTick(() => {
    if (!current()) return;
    focusInput();
    if (document.activeElement === searchInputRef.value) searchInputRef.value?.select();
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
  activatePane();
  if (id === "split-h") emit("split-h");
  else if (id === "split-v") emit("split-v");
  else if (id === "search") openSearch();
  else if (id === "fullscreen") toggleFullscreen();
  else if (id === "reconnect") { queueFocus(); void nextTick(() => connectSession()); }
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
      <button v-if="!pane.connected" class="shrink-0 text-primary disabled:opacity-50" :disabled="connecting" @click.stop="reconnect">Reconnect</button>
    </div>
    <div ref="containerRef" class="min-h-0 flex-1 overflow-hidden" />
    <SshPasswordPrompt ref="passwordPrompt" :active="isActive" @activate="activatePane()" @finished="queueFocus()" />
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
