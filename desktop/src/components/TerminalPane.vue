<script setup lang="ts">
import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";
import { useTabsStore, isPane, type Pane, type DropPosition } from "../stores/tabs";
import { setDragImageChip } from "../lib/dragChip";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { useVaultStore } from "../stores/vault";
import { useUiStore } from "../stores/ui";
import { useSettingsStore } from "../stores/settings";
import ActionMenu, { type MenuAction } from "./ui/ActionMenu.vue";
import SshPasswordPrompt from "./SshPasswordPrompt.vue";
import { useFocusIntent } from "../composables/useFocusIntent";
import { acquireWebglRenderer, releaseWebglRenderer } from "../lib/terminalRenderer";
import { configuredSshEndpoint, effectiveSshIdentity, formatSshEndpoint, passwordAuth, resolvedSshHost, sshConfigurationKey, sshIdentityReady } from "../lib/sshIdentity";
import { useMaskedAddress } from "../composables/useMaskedAddress";
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
const settings = useSettingsStore();
const { maskAddress } = useMaskedAddress();
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
const searchResultIndex = ref(-1);
const searchResultCount = ref(0);
const connectedEndpoint = ref<string | null>(props.pane.terminalType === "local" ? "Local shell" : null);
let term: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let searchAddon: SearchAddon | null = null;
let observer: ResizeObserver | null = null;
const listeners: UnlistenFn[] = [];
let listenerSetup: Promise<boolean> | null = null;
let currentSessionId: string | null = null;
let disposed = false;
let sessionEnded = false;
// Buffer remote output, never user input, until this attempt can answer queries.
let pendingOutput: Uint8Array[] | null = null;
const listenersReady = ref(false);
const listenerSetupPending = ref(false);
const isActive = computed(() => props.visible && tabs.activeTabId === props.tabId && tabs.activePaneId === props.pane.id);
const isFullscreen = computed(() => ui.fullscreenPaneId === props.pane.id);
// Keep obscured terminal owners alive, but out of keyboard/pointer navigation.
const inputObscured = computed(() => !props.visible || (!!ui.fullscreenPaneId && !isFullscreen.value));
const isDragging = computed(() => tabs.draggedPaneId === props.pane.id);
const isDragOver = computed(() => tabs.dragOverPaneId === props.pane.id);
const busy = computed(() => connecting.value || listenerSetupPending.value);
const status = computed(() => busy.value ? "Connecting" : props.pane.connected ? "Connected" : "Disconnected");
const otherTabs = computed(() => tabs.tabs.filter(t => t.id !== props.tabId));
const searchSummary = computed(() => {
  if (!searchQuery.value) return "";
  if (searchResultCount.value === 0) return "No results";
  const index = searchResultIndex.value < 0 ? 0 : searchResultIndex.value;
  return `${index + 1} of ${searchResultCount.value}`;
});

const configuredEndpoint = computed(() => {
  if (props.pane.terminalType === "local") return "Local shell";
  const host = hosts.hosts.find(item => item.id === props.pane.hostId);
  if (!host) return props.pane.title;
  if (!sshIdentityReady(host, identities.loaded)) return `Resolving SSH identity · ${host.hostname}:${host.port}`;
  return configuredSshEndpoint(host, identities.identities);
});
const hostAddress = computed(() => connectedEndpoint.value ?? configuredEndpoint.value);
const displayAddress = computed(() => maskAddress(hostAddress.value));

function focusInput() {
  if (disposed || !isActive.value || inputObscured.value || terminalFocusBlocked()) return;
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
// Teleporting the pane to <body> on fullscreen changes its DOM location; refit
// the terminal immediately after the post-flush DOM update so xterm fills the
// new viewport-sized container without a blank frame.
watch(isFullscreen, () => { fit(); queueFocus(); }, { flush: "post" });
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
  void connectWhenReady();
}
function writeError(message: string) {
  error.value = message;
  term?.write(`\r\n\x1b[31m${message}\x1b[0m\r\n`);
}
async function ensureListeners(): Promise<boolean> {
  if (disposed) return false;
  if (listenersReady.value) return true;
  if (listenerSetup) return listenerSetup;
  listenerSetupPending.value = true;
  listenerSetup = (async () => {
    const staged: UnlistenFn[] = [];
    try {
      const dataListener = await api.onSessionData(event => {
        if (disposed || props.pane.closing || sessionEnded || event.session_id !== currentSessionId) return;
        const data = new Uint8Array(event.data);
        if (pendingOutput) pendingOutput.push(data);
        else if (props.pane.connected) term?.write(data);
      });
      staged.push(dataListener);
      if (disposed) { staged.forEach(unlisten => unlisten()); return false; }
      const closeListener = await api.onSessionClosed(event => {
        if (!disposed && !props.pane.closing && !sessionEnded && event.session_id === currentSessionId) {
          sessionEnded = true;
          tabs.setPaneDisconnected(props.pane.id);
          // EOF does not invalidate diagnostics already received from this owner.
          // Disable writes before parsing them: queries must not reply to a dead
          // session. Reconnect still drains this queue before resetting xterm.
          const output = pendingOutput;
          pendingOutput = null;
          for (const chunk of output ?? []) term?.write(chunk);
          writeError(`Session closed: ${event.reason}`);
        }
      });
      staged.push(closeListener);
      if (disposed) { staged.forEach(unlisten => unlisten()); return false; }
      listeners.push(...staged);
      listenersReady.value = true;
      return true;
    } catch (cause) {
      staged.forEach(unlisten => unlisten());
      if (!disposed) writeError(`Could not initialize terminal: ${String(cause)}`);
      return false;
    } finally {
      listenerSetupPending.value = false;
      listenerSetup = null;
    }
  })();
  return listenerSetup;
}
async function connectWhenReady() {
  if (disposed || connecting.value) return;
  if (await ensureListeners()) await connectSession();
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
      // Direct hosts do not depend on the identity list. Linked hosts still fail
      // closed, and every async boundary re-reads configuration before dispatch.
      const resolveHost = async () => {
        while (!disposed && !props.pane.closing) {
          const host = hosts.hosts.find(h => h.id === props.pane.hostId);
          if (!host) throw new Error("Host not found. Check the saved host configuration.");
          if (sshIdentityReady(host, identities.loaded)) {
            const effective = effectiveSshIdentity(host, identities.identities);
            if (effective.missing) {
              throw new Error("Linked SSH identity not found. Repair the host configuration before reconnecting.");
            }
            return host;
          }
          await identities.ensureLoaded();
        }
      };
      const host = await resolveHost();
      if (!host || disposed || props.pane.closing) return;
      const auth = effectiveSshIdentity(host, identities.identities).auth;
      if (auth === "publickey" && !vault.unlocked) {
        if (!await ui.requestVaultUnlock()) throw new Error("Connection cancelled: vault remains locked.");
        if (disposed) return;
      }
      const openSsh = async () => {
        // Re-read after unlock: an edited direct host may now link an identity.
        const host = await resolveHost();
        if (!host || disposed || props.pane.closing) return;
        const effective = effectiveSshIdentity(host, identities.identities);
        const key = sshConfigurationKey(host, identities.identities);
        const transportHost = resolvedSshHost(host, identities.identities);
        endpointForAttempt = configuredSshEndpoint(host, identities.identities);
        let password: string | null = null;
        try {
          if (passwordAuth(effective.auth)) {
            if (!passwordPrompt.value) throw new Error("Password prompt is not ready. Reconnect to retry.");
            password = await passwordPrompt.value.request(endpointForAttempt, mayAutofocusPassword);
            if (disposed || props.pane.closing) return;
            if (password === null) throw new Error("Connection cancelled.");
            const latest = hosts.hosts.find(h => h.id === props.pane.hostId);
            if (!latest || !sshIdentityReady(latest, identities.loaded) || sshConfigurationKey(latest, identities.identities) !== key)
              throw new Error("Connection settings changed. Reconnect to review the updated account.");
          }
          const request = api.connectSsh(sessionId, transportHost, password, terminal.cols, terminal.rows, effective.username);
          // The IPC request owns its serialized argument; retain no reusable credential.
          password = null;
          const connected = await request;
          // Rust returns metadata from the same immutable transport snapshot used
          // to authenticate; later identity edits cannot change this attempt.
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
    cursorBlink: true, scrollback: 10000, allowProposedApi: true,
    // Builds xterm's accessibility layer: its own row elements and a live
    // region, read from the buffer rather than from whatever is drawing, so it
    // works the same on the GPU renderer as on the DOM one.
    screenReaderMode: settings.screenReaderMode });
  fitAddon = new FitAddon();
  searchAddon = new SearchAddon();
  term.loadAddon(fitAddon);
  term.loadAddon(searchAddon);
  searchAddon.onDidChangeResults(event => {
    searchResultIndex.value = event.resultIndex;
    searchResultCount.value = event.resultCount;
  });
  term.attachCustomKeyEventHandler(event => {
    // xterm calls stopPropagation() for Escape by default, which prevents the
    // window-level Escape handler in App.vue from firing. Return false while
    // fullscreen so the event bubbles up and exits fullscreen without being
    // sent to the remote shell.
    if (event.key === "Escape" && isFullscreen.value) return false;
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
    const sessionId = currentSessionId;
    if (!disposed && !props.pane.closing && !sessionEnded && sessionId && props.pane.connected && !connecting.value)
      void api.sessionWrite(sessionId, Array.from(new TextEncoder().encode(data))).catch(cause => {
        // An old write may settle after EOF or after xterm has a new session.
        // Preserve that owner's transcript/status; only report a live owner's error.
        if (!disposed && !props.pane.closing && !sessionEnded && currentSessionId === sessionId && props.pane.connected && !connecting.value)
          writeError(`Write failed: ${String(cause)}`);
      });
  });
  observer = new ResizeObserver(() => fit());
  observer.observe(containerRef.value);
  fit();
  if (props.visible) void acquireWebglRenderer(props.pane.id, term);
  // Wait for ancestor v-show updates; the mount hook can run while still hidden.
  // This ticket belongs to creation, not to later network completion.
  queueFocus();
  // Directly created panes auto-connect. Workspace restore can explicitly opt an
  // SSH pane out; Reconnect then initializes listeners and starts it on demand.
  if (props.pane.autoConnect !== false) await connectWhenReady();
});
// Claim the GPU renderer whenever this pane comes to the front, so the budget
// tracks the terminals in use. Panes that lose the claim keep working on
// xterm's DOM renderer.
watch([() => props.visible, isActive], ([visible, active], [wasVisible, wasActive]) => {
  if (term && ((visible && !wasVisible) || (active && !wasActive))) void acquireWebglRenderer(props.pane.id, term);
});
// Turning screen reader support on has to reach terminals that are already
// open, or it would only apply to panes created afterwards.
watch(() => settings.screenReaderMode, enabled => {
  if (term) term.options.screenReaderMode = enabled;
});
onBeforeUnmount(() => {
  disposed = true;
  pendingOutput = null;
  passwordPrompt.value?.cancel();
  listeners.forEach(unlisten => unlisten());
  observer?.disconnect();
  // Hand back the slot while the terminal can still be reverted to its DOM renderer.
  releaseWebglRenderer(props.pane.id);
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
  searchResultIndex.value = -1;
  searchResultCount.value = 0;
  focusInput();
}
function toggleFullscreen() {
  activatePane();
  ui.toggleFullscreen(props.pane.id);
}
function doSearch(previous = false) {
  if (!searchQuery.value) {
    searchAddon?.clearDecorations();
    searchResultIndex.value = -1;
    searchResultCount.value = 0;
    return;
  }
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
  setDragImageChip(event, props.pane.title);
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
// A dragged tab merges its tree into the hovered pane; a single-pane tab can
// also swap with it. The center label reflects which will happen.
const centerDropLabel = computed(() => {
  const tab = tabs.tabs.find(t => t.id === tabs.draggedTabId);
  return tab && !isPane(tab.tree) ? "Merge" : "Swap";
});
function dragOver(event: DragEvent) {
  if ((!tabs.draggedPaneId && !tabs.draggedTabId) || isDragging.value || !paneRef.value) return;
  // A tab cannot be dropped onto one of its own panes.
  if (tabs.draggedTabId && tabs.owningTab(props.pane.id)?.id === tabs.draggedTabId) return;
  event.preventDefault();
  tabs.setDragOver(props.pane.id, dropPosition(event));
}
function drop(event: DragEvent) {
  if ((!tabs.draggedPaneId && !tabs.draggedTabId) || !paneRef.value) return;
  event.preventDefault();
  // Reveal the resulting split instead of keeping the old pane fullscreened.
  ui.exitFullscreen();
  if (tabs.draggedTabId) tabs.dropTabOnPane(tabs.draggedTabId, props.pane.id, dropPosition(event));
  else tabs.dropPane(props.pane.id, dropPosition(event));
}
const actions = computed<MenuAction[]>(() => [
  { id: "split-h", label: "Split right…", icon: SplitSquareHorizontal },
  { id: "split-v", label: "Split below…", icon: SplitSquareVertical },
  { id: "search", label: "Find in terminal", icon: Search, shortcut: "⌘ / Ctrl F", separator: true },
  { id: "fullscreen", label: isFullscreen.value ? "Restore pane" : "Maximize pane", icon: isFullscreen.value ? Minimize2 : Maximize2 },
  ...otherTabs.value.map((tab, index) => ({ id: `move:${tab.id}`, label: `Move to ${tab.title}`, icon: ArrowUpRight, separator: index === 0 })),
  ...(!props.pane.connected ? [{ id: "reconnect", label: "Reconnect", icon: RotateCw, disabled: busy.value, separator: true }] : []),
  { id: "close", label: "Close session", icon: X, danger: true, separator: true },
]);
function selectAction(id: string) {
  activatePane();
  if (id === "split-h") emit("split-h");
  else if (id === "split-v") emit("split-v");
  else if (id === "search") openSearch();
  else if (id === "fullscreen") toggleFullscreen();
  else if (id === "reconnect") { queueFocus(); void nextTick(() => connectWhenReady()); }
  else if (id === "close") emit("close");
  else if (id.startsWith("move:")) tabs.movePaneToTab(props.pane.id, id.slice(5));
}
</script>

<template>
  <Teleport to="body" :disabled="!isFullscreen">
  <div ref="paneRef" class="terminal-pane flex h-full w-full min-w-0 flex-col" :inert="inputObscured"
    :class="[isFullscreen ? 'fixed inset-0 z-[90]' : 'relative', isActive ? 'ring-1 ring-inset ring-ring/50' : '', isDragging ? 'pane-dragging' : '', isDragOver ? 'pane-drop-target' : '']"
    :data-session-id="pane.sessionId" :data-connected="pane.connected" :data-host-id="pane.hostId" :data-active="isActive"
    @click="focusPane" @dragover="dragOver" @drop="drop"
    @dragleave="!paneRef?.contains($event.relatedTarget as Node) && tabs.clearDragOver()">
    <header class="pane-header" :class="{ 'pane-header-active': isActive }" data-testid="pane-header">
      <div class="flex min-w-0 flex-1 cursor-grab items-center gap-2 active:cursor-grabbing" draggable="true"
        :aria-label="`Drag pane ${pane.title}`" @dragstart="startDrag" @dragend="tabs.endDrag()">
        <GripVertical class="size-3 shrink-0 text-muted-foreground" />
        <Loader2 v-if="busy" class="size-3 shrink-0 animate-spin text-muted-foreground" aria-label="Connecting" />
        <span v-else class="size-1.5 shrink-0 rounded-full" :class="pane.connected ? 'bg-emerald-500' : 'bg-muted-foreground'" :title="status" />
        <span class="truncate text-[12px]" :title="`${pane.title} · ${displayAddress} · ${status}`">{{ displayAddress }}</span>
        <span class="sr-only" role="status">{{ status }}</span>
      </div>
      <div class="flex shrink-0 items-center gap-0.5 text-muted-foreground" @click.stop
        @pointerdown.capture="activatePane()" @focusin="activatePane()">
        <button class="pane-button" aria-label="Search in terminal" title="Search (Cmd/Ctrl+F)" @click="openSearch"><Search class="size-3.5" /></button>
        <button class="pane-button" :aria-label="isFullscreen ? 'Exit fullscreen' : 'Fullscreen'" :title="isFullscreen ? 'Restore pane (Escape)' : 'Maximize pane'" @click="toggleFullscreen"><Minimize2 v-if="isFullscreen" class="size-3.5" /><Maximize2 v-else class="size-3.5" /></button>
        <ActionMenu :label="`Actions for ${pane.title}`" :items="actions" :enabled="!inputObscured" @select="selectAction" />
      </div>
    </header>
    <div v-if="error" class="flex shrink-0 items-center gap-2 border-b border-border bg-background px-3 py-2 text-xs" role="alert">
      <span class="min-w-0 flex-1 text-muted-foreground">{{ error }}</span>
      <button v-if="!pane.connected" class="shrink-0 text-primary disabled:opacity-50" :disabled="busy" @click.stop="reconnect">Reconnect</button>
    </div>
    <!-- Tab/Shift+Tab can enter xterm without a click. Publish ownership before
         any subsequent key is routed; do not gate background protocol replies. -->
    <div ref="containerRef" class="min-h-0 flex-1 overflow-hidden" @focusin="activatePane()" />
    <SshPasswordPrompt ref="passwordPrompt" :active="isActive" @activate="activatePane()" @finished="queueFocus()" />
    <div v-if="showSearch" class="absolute right-2 top-10 z-40 flex max-w-[calc(100%-16px)] flex-wrap items-center gap-1 rounded-md border border-border bg-background p-1 shadow-lg" @click.stop @keydown.stop="searchKey" @focusin="activatePane()">
      <input ref="searchInputRef" v-model="searchQuery" aria-label="Search terminal output" placeholder="Search..." class="h-7 w-36 min-w-0 bg-transparent px-2 text-xs outline-none" @input="doSearch()" />
      <span v-if="searchSummary" class="px-1 text-[11px] tabular-nums whitespace-nowrap select-none" :class="searchResultCount === 0 ? 'text-destructive' : 'text-muted-foreground'" aria-live="polite">{{ searchSummary }}</span>
      <button class="pane-button" :aria-pressed="searchCaseSensitive" aria-label="Case sensitive" @click="searchCaseSensitive = !searchCaseSensitive; doSearch()"><CaseSensitive class="size-3.5" /></button>
      <button class="pane-button" :aria-pressed="searchWholeWord" aria-label="Whole word" @click="searchWholeWord = !searchWholeWord; doSearch()"><WholeWord class="size-3.5" /></button>
      <button class="pane-button" :aria-pressed="searchRegex" aria-label="Regular expression" @click="searchRegex = !searchRegex; doSearch()"><Regex class="size-3.5" /></button>
      <button class="pane-button" aria-label="Previous match" @click="doSearch(true)"><ChevronUp class="size-3.5" /></button>
      <button class="pane-button" aria-label="Next match" @click="doSearch()"><ChevronDown class="size-3.5" /></button>
      <button class="pane-button" aria-label="Close search" @click="closeSearch"><X class="size-3.5" /></button>
    </div>
    <div v-if="isDragOver && (tabs.draggedPaneId || tabs.draggedTabId) && !isDragging" class="drop-overlay" aria-hidden="true">
      <div class="drop-zone drop-zone-top" :class="{ 'drop-zone-active': tabs.dragOverPosition === 'top' }"><span>Split above</span></div>
      <div class="drop-zone drop-zone-bottom" :class="{ 'drop-zone-active': tabs.dragOverPosition === 'bottom' }"><span>Split below</span></div>
      <div class="drop-zone drop-zone-left" :class="{ 'drop-zone-active': tabs.dragOverPosition === 'left' }"><span>Split left</span></div>
      <div class="drop-zone drop-zone-right" :class="{ 'drop-zone-active': tabs.dragOverPosition === 'right' }"><span>Split right</span></div>
      <div class="drop-zone drop-zone-center" :class="{ 'drop-zone-active': tabs.dragOverPosition === 'center' }"><span>{{ centerDropLabel }}</span></div>
    </div>
  </div>
  </Teleport>
</template>

<style scoped>
.terminal-pane { background: var(--terminal-background); transition: box-shadow 180ms ease-out, opacity 160ms ease-out; }
.pane-dragging { opacity: 0.5; }
.pane-drop-target { box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--workspace-accent) 70%, transparent); }
.pane-header { @apply flex h-9 shrink-0 items-center gap-2 border-b px-2 text-muted-foreground; background: var(--terminal-toolbar); border-color: var(--terminal-border); }
.pane-header-active { background: var(--terminal-toolbar-active); color: hsl(var(--foreground)); box-shadow: inset 2px 0 var(--workspace-accent); }
.pane-button { @apply flex size-8 items-center justify-center rounded-md text-current hover:bg-muted disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring; }
.pane-button[aria-pressed="true"] { @apply bg-primary/20 text-primary; }
/* Directional drop zones: four edges split the pane, the center swaps it.
   The overlay never captures pointer events so dragover/drop reach the pane.
   Each zone's tint wipes in from the edge the pane will land on and its label
   slides in from the same direction, previewing the split rather than naming
   it. Transitions retarget mid-flight as the pointer crosses zones. */
.drop-overlay { @apply pointer-events-none absolute inset-0 z-30; animation: drop-overlay-in 140ms ease-out; }
@keyframes drop-overlay-in { from { opacity: 0; } }
.drop-zone { @apply absolute flex items-center justify-center opacity-0; transition: opacity 140ms ease-out; }
.drop-zone::before { content: ""; @apply absolute inset-0; transition: clip-path 160ms cubic-bezier(0.23, 1, 0.32, 1); }
.drop-zone span { @apply rounded-full bg-primary/90 px-2.5 py-1 text-[11px] font-medium text-primary-foreground opacity-0 shadow-lg backdrop-blur-sm; transition: transform 160ms cubic-bezier(0.23, 1, 0.32, 1), opacity 140ms ease-out; }
.drop-zone-active { @apply opacity-100; }
.drop-zone-top { @apply left-0 right-0 top-0 h-1/3; }
.drop-zone-bottom { @apply left-0 right-0 bottom-0 h-1/3; }
.drop-zone-left { @apply left-0 top-0 bottom-0 w-1/3; }
.drop-zone-right { @apply right-0 top-0 bottom-0 w-1/3; }
.drop-zone-center { @apply inset-1/3; }
/* Zone previews share the workspace accent used by the active-pane underline
   and the reorder caret, so every drag affordance reads as one system. */
.drop-zone-top::before, .drop-zone-bottom::before, .drop-zone-left::before, .drop-zone-right::before { background: color-mix(in srgb, var(--workspace-accent) 20%, transparent); }
.drop-zone-center::before { background: color-mix(in srgb, var(--workspace-accent) 28%, transparent); border-radius: 8px; }
.drop-zone-top::before { clip-path: inset(0 0 100% 0); }
.drop-zone-bottom::before { clip-path: inset(100% 0 0 0); }
.drop-zone-left::before { clip-path: inset(0 100% 0 0); }
.drop-zone-right::before { clip-path: inset(0 0 0 100%); }
.drop-zone-center::before { clip-path: inset(40% round 8px); }
.drop-zone-top.drop-zone-active::before, .drop-zone-bottom.drop-zone-active::before,
.drop-zone-left.drop-zone-active::before, .drop-zone-right.drop-zone-active::before { clip-path: inset(0); }
.drop-zone-center.drop-zone-active::before { clip-path: inset(0 round 8px); }
.drop-zone-top span { transform: translateY(-8px) scale(0.92); }
.drop-zone-bottom span { transform: translateY(8px) scale(0.92); }
.drop-zone-left span { transform: translateX(-8px) scale(0.92); }
.drop-zone-right span { transform: translateX(8px) scale(0.92); }
.drop-zone-center span { transform: scale(0.9); }
.drop-zone-active span { opacity: 1; transform: none; }
</style>