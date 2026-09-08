<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { Server, TerminalSquare, X, Search, ArrowUpRight } from "lucide-vue-next";
import { collectPanes, useTabsStore } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { configuredSshEndpoint, effectiveSshIdentity, linkedSshIdentityMissing, sshIdentityReady } from "../lib/sshIdentity";
import { useMaskedAddress } from "../composables/useMaskedAddress";
import type { Host } from "../types";

export type SessionPlacement = "tab" | "horizontal" | "vertical";
const emit = defineEmits<{ opened: [] }>();
const tabs = useTabsStore();
const hosts = useHostsStore();
const identities = useIdentitiesStore();
const { maskAddress } = useMaskedAddress();
const dialog = ref<HTMLDialogElement | null>(null);
const search = ref<HTMLInputElement | null>(null);
const query = ref("");
const placement = ref<SessionPlacement>("tab");
const targetId = ref<string | null>(null);
const target = computed(() => tabs.tabs.flatMap(tab => collectPanes(tab.tree)).find(pane => pane.id === targetId.value));
function identityReady(host: Host) {
  return sshIdentityReady(host, identities.loaded) &&
    !linkedSshIdentityMissing(host, identities.identities);
}
const results = computed(() => {
  const needle = query.value.trim().toLowerCase();
  return hosts.hosts.filter(host => {
    const username = identityReady(host) ? effectiveSshIdentity(host, identities.identities).username : "";
    return `${host.label} ${host.hostname} ${username} ${host.tags.join(" ")}`.toLowerCase().includes(needle);
  });
});
const localMatches = computed(() => !query.value || "local shell terminal".includes(query.value.trim().toLowerCase()));
async function loadIdentities() {
  try { await identities.ensureLoaded(); } catch { /* Store exposes a retryable error. */ }
}
async function show(where: SessionPlacement = "tab", paneId: string | null = tabs.activePaneId) {
  query.value = "";
  targetId.value = paneId;
  placement.value = target.value ? where : "tab";
  dialog.value?.showModal();
  await nextTick();
  search.value?.focus();
  await loadIdentities();
}
function connect(host?: Host) {
  if (host && !identityReady(host)) return;
  // Finish native focus restoration before selecting the new destination.
  dialog.value?.close();
  if (placement.value === "tab" || !target.value) tabs.newTab(host);
  else tabs.splitPane(target.value.id, placement.value, host);
  emit("opened");
}
function enterSearch(event: KeyboardEvent) {
  if (event.key !== "Enter" || event.isComposing) return;
  event.preventDefault();
  if (results.value.length === 1 && !localMatches.value) connect(results.value[0]);
  else if (results.value.length === 0 && localMatches.value) connect();
}
defineExpose({ show });
</script>

<template>
  <dialog ref="dialog" class="session-picker" aria-labelledby="session-picker-heading" @keydown.stop>
    <header class="flex items-center justify-between px-5 pb-3 pt-5">
      <div><h2 id="session-picker-heading" class="text-base font-semibold">New session</h2>
        <p class="mt-1 text-xs text-muted-foreground">A saved host or a local shell. Your other sessions stay open.</p></div>
      <button class="ml-3 flex size-8 shrink-0 items-center justify-center rounded-md hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
        aria-label="Close session picker" @click="dialog?.close()"><X class="size-4" /></button>
    </header>
    <div class="space-y-3 px-5 pb-4">
      <label class="flex items-center gap-3 text-xs text-muted-foreground">Open in
        <select v-model="placement" class="min-w-0 flex-1 rounded-lg border border-border bg-background px-3 py-2 text-[13px] text-foreground focus-visible:ring-2 focus-visible:ring-ring" aria-label="Open session in">
          <option value="tab">New tab</option>
          <option value="horizontal" :disabled="!target">Split right of {{ target?.title ?? 'active pane' }}</option>
          <option value="vertical" :disabled="!target">Split below {{ target?.title ?? 'active pane' }}</option>
        </select>
      </label>
      <div class="flex items-center gap-2 rounded-lg border border-border bg-background px-3 focus-within:ring-2 focus-within:ring-ring">
        <Search class="size-4 text-muted-foreground" />
        <input ref="search" v-model="query" aria-label="Search sessions" placeholder="Search hosts, addresses or tags…"
          class="h-10 min-w-0 flex-1 bg-transparent text-sm outline-none" @keydown="enterSearch" />
      </div>
      <p v-if="identities.loading" class="text-xs text-muted-foreground" role="status">Loading linked SSH identities… Direct hosts and local shells remain available.</p>
      <div v-else-if="identities.loadError" class="text-xs text-destructive" role="alert">
        {{ identities.loadError }} Direct hosts and local shells remain available.
        <button class="ml-2 underline" @click="loadIdentities">Retry identities</button>
      </div>
    </div>
    <div class="max-h-[min(380px,50dvh)] overflow-y-auto border-t border-border px-3 py-2">
      <button v-if="localMatches" class="session-choice" aria-label="Open local shell" @click="connect()">
        <span class="session-avatar bg-muted text-muted-foreground"><TerminalSquare class="size-4" /></span>
        <span class="min-w-0 flex-1"><span class="block text-[13px] font-medium">Local shell</span><span class="block text-xs text-muted-foreground">On this device</span></span>
        <ArrowUpRight class="size-4 text-muted-foreground" />
      </button>
      <div v-if="results.length" class="px-3 pb-1 pt-3 text-[11px] font-medium text-muted-foreground">Saved hosts</div>
      <button v-for="host in results" :key="host.id" class="session-choice disabled:opacity-50" :disabled="!identityReady(host)" :aria-label="`Connect ${host.label}`" @click="connect(host)">
        <span class="session-avatar bg-blue-500/10 text-blue-500"><Server class="size-4" /></span>
        <span class="min-w-0 flex-1"><span class="block truncate text-[13px] font-medium">{{ host.label }}</span>
          <span class="block truncate text-xs text-muted-foreground">{{ sshIdentityReady(host, identities.loaded) ? maskAddress(configuredSshEndpoint(host, identities.identities)) : 'SSH identity not yet resolved' }}</span></span>
        <ArrowUpRight class="size-4 text-muted-foreground" />
      </button>
      <p v-if="!results.length && !localMatches" class="px-3 py-8 text-center text-sm text-muted-foreground">No matching sessions. Try another name or address.</p>
      <p v-if="!hosts.hosts.length" class="px-3 py-4 text-xs text-muted-foreground">Save a connection in Hosts to see it here.</p>
    </div>
    <footer class="border-t border-border px-5 py-3 text-[11px] text-muted-foreground">Escape to cancel · Enter to open a single search result</footer>
  </dialog>
</template>

<style scoped>
.session-picker { @apply w-[min(480px,calc(100vw-32px))] max-h-[calc(100dvh-32px)] overflow-y-auto rounded-2xl border border-border bg-background p-0 text-foreground shadow-2xl backdrop:bg-black/50; }
.session-choice { @apply flex min-h-14 w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring; }
.session-avatar { @apply flex size-9 shrink-0 items-center justify-center rounded-xl; }
</style>
