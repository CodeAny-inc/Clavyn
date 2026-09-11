<script setup lang="ts">
import { ref, computed, onMounted } from "vue";
import * as api from "../api";
import { useMaskedAddress } from "../composables/useMaskedAddress";
import Input from "./ui/Input.vue";
import Badge from "./ui/Badge.vue";
import Button from "./ui/Button.vue";
import {
  ShieldCheck,
  ShieldAlert,
  ShieldOff,
  Trash2,
  Search,
  Fingerprint,
} from "lucide-vue-next";
import type { KnownHostEntry, PendingHostKeyChange } from "../types";

const hosts = ref<KnownHostEntry[]>([]);
const removedHosts = ref<KnownHostEntry[]>([]);
const changes = ref<PendingHostKeyChange[]>([]);
const search = ref("");
const loading = ref(false);
const { maskAddress } = useMaskedAddress();

onMounted(async () => {
  await load();
});

async function load() {
  loading.value = true;
  try {
    const [known, removed, pending] = await Promise.all([
      api.listKnownHosts(),
      api.listRemovedKnownHosts(),
      api.listHostKeyChanges(),
    ]);
    hosts.value = known;
    removedHosts.value = removed;
    changes.value = pending;
  } finally {
    loading.value = false;
  }
}

const filteredHosts = computed(() => {
  if (!search.value.trim()) return hosts.value;
  const q = search.value.toLowerCase();
  return hosts.value.filter(
    (h) =>
      h.host.toLowerCase().includes(q) ||
      h.fingerprint.toLowerCase().includes(q),
  );
});

function parseHostPort(entry: string): [string, number] {
  // Entries are written as "host:port" and may also arrive bracketed as
  // "[host]:port". Split on the last colon rather than the only one, so a bare
  // IPv6 address keeps its own colons: "::1:22" is ::1 on port 22.
  const bracketed = entry.match(/^\[([^\]]+)\]:(\d+)$/);
  if (bracketed) return [bracketed[1], parseInt(bracketed[2], 10)];
  const lastColon = entry.lastIndexOf(":");
  if (lastColon > 0 && /^\d+$/.test(entry.slice(lastColon + 1))) {
    return [entry.slice(0, lastColon), parseInt(entry.slice(lastColon + 1), 10)];
  }
  return [entry, 22];
}

async function remove(entry: KnownHostEntry) {
  const [host, port] = parseHostPort(entry.host);
  if (confirm(`Remove known host "${entry.host}"?`)) {
    await api.removeKnownHost(host, port);
    await load();
  }
}

const forgetError = ref("");

async function forget(entry: KnownHostEntry) {
  const [host, port] = parseHostPort(entry.host);
  const confirmed = confirm(
    `Permanently forget "${entry.host}"?\n\n` +
      `Clavyn still remembers the key this host was pinned to (${entry.fingerprint}), ` +
      "which is how it can tell a changed key from a new one.\n\n" +
      "Forgetting it erases that record. The next connection to this host will " +
      "trust whatever key it is offered, with no warning.",
  );
  if (!confirmed) return;
  forgetError.value = "";
  try {
    await api.forgetKnownHost(host, port);
  } catch (cause) {
    forgetError.value = String(cause);
  }
  await load();
}

const trustError = ref("");

async function trust(change: PendingHostKeyChange) {
  const [host, port] = parseHostPort(change.host);
  const confirmed = confirm(
    `The host key for "${change.host}" changed.\n\n` +
      `Pinned:    ${change.pinned_fingerprint}\n` +
      `Presented: ${change.presented_fingerprint}\n\n` +
      "Trust the presented key only if you can confirm the change with the server's operator.",
  );
  if (!confirmed) return;
  trustError.value = "";
  try {
    // Send back the fingerprint that was shown, so a key that arrived after this
    // view rendered cannot be trusted on the strength of the old one.
    await api.replaceKnownHost(host, port, change.presented_fingerprint);
  } catch (cause) {
    trustError.value = String(cause);
  }
  await load();
}
</script>

<template>
  <div class="flex flex-col h-full overflow-hidden">
    <!-- Header -->
    <div class="flex h-11 items-center gap-2 border-b border-border px-4 pl-12 md:pl-4">
      <h2 class="text-[14px] font-semibold truncate">Known Hosts</h2>
      <div class="ml-auto text-[12px] text-muted-foreground shrink-0">
        {{ hosts.length }} host{{ hosts.length === 1 ? '' : 's' }}
      </div>
    </div>

    <!-- Search -->
    <div class="px-3 py-2 border-b border-border">
      <div class="relative">
        <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" :stroke-width="1.75" />
        <Input v-model="search" placeholder="Search by hostname or fingerprint..." class="pl-8" />
      </div>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto p-3">
      <!-- Key changes awaiting review. A host listed here is refusing to connect
           until the presented key is trusted or the server is fixed. Held back
           while a reload is in flight, so a stale list never sits above the
           loading indicator. -->
      <div v-if="!loading && changes.length" class="mb-3 flex flex-col gap-1.5">
        <div class="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
          Host key changes
        </div>
        <div
          v-for="change in changes"
          :key="change.host"
          class="flex items-start gap-3 rounded-md border border-destructive/40 bg-destructive/5 p-3"
          data-testid="host-key-change"
        >
          <div class="flex h-9 w-9 items-center justify-center rounded-md shrink-0 bg-destructive/10">
            <ShieldAlert class="size-4 text-destructive" :stroke-width="1.75" />
          </div>
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2">
              <span class="text-[13px] font-medium truncate font-mono">{{ maskAddress(change.host) }}</span>
              <Badge>{{ change.key_type }}</Badge>
            </div>
            <p class="mt-1 text-[12px] text-foreground">
              The key this host presents is not the pinned one. Connections stay
              blocked until you confirm the change with whoever runs the server.
            </p>
            <div class="mt-1.5 flex flex-col gap-0.5 text-[11px] text-muted-foreground">
              <span class="font-mono truncate">Pinned: {{ change.pinned_fingerprint }}</span>
              <span class="font-mono truncate">Presented: {{ change.presented_fingerprint }}</span>
            </div>
          </div>
          <Button variant="outline" size="sm" class="shrink-0" @click="trust(change)">
            Trust new key
          </Button>
        </div>
        <p v-if="trustError" class="text-[12px] text-destructive">{{ trustError }}</p>
      </div>

      <!-- Removed hosts whose last key is still retained. Kept visible so the
           record can be erased from here instead of by hand. -->
      <div v-if="!loading && removedHosts.length" class="mb-3 flex flex-col gap-1.5">
        <div class="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
          Removed hosts
        </div>
        <div
          v-for="entry in removedHosts"
          :key="entry.host"
          class="flex items-start gap-3 rounded-md border border-border bg-card p-3"
          data-testid="removed-known-host"
        >
          <div class="flex h-9 w-9 items-center justify-center rounded-md shrink-0 bg-muted">
            <ShieldOff class="size-4 text-muted-foreground" :stroke-width="1.75" />
          </div>
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2">
              <span class="text-[13px] font-medium truncate font-mono">{{ maskAddress(entry.host) }}</span>
              <Badge>{{ entry.key_type }}</Badge>
            </div>
            <p class="mt-1 text-[12px] text-muted-foreground">
              No longer trusted. The key it was pinned to is still remembered, so
              a different key is reported as a change rather than a first contact.
            </p>
            <div class="flex items-center gap-1.5 mt-1 text-[11px] text-muted-foreground">
              <Fingerprint class="size-3" :stroke-width="1.75" />
              <span class="font-mono truncate">{{ entry.fingerprint }}</span>
            </div>
          </div>
          <Button variant="outline" size="sm" class="shrink-0" @click="forget(entry)">
            Forget permanently
          </Button>
        </div>
        <p v-if="forgetError" class="text-[12px] text-destructive">{{ forgetError }}</p>
      </div>

      <div v-if="loading" class="py-12 text-center text-[13px] text-muted-foreground">Loading...</div>

      <div v-else-if="filteredHosts.length" class="flex flex-col gap-1.5">
        <div
          v-for="(host, i) in filteredHosts"
          :key="i"
          class="group flex items-start gap-3 rounded-md border border-border bg-card p-3 transition-colors duration-100 hover:border-muted-foreground/30"
        >
          <div class="flex h-9 w-9 items-center justify-center rounded-md shrink-0 bg-green-500/10">
            <ShieldCheck class="size-4 text-green-500" :stroke-width="1.75" />
          </div>
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2">
              <span class="text-[13px] font-medium truncate font-mono">{{ maskAddress(host.host) }}</span>
              <Badge>{{ host.key_type }}</Badge>
            </div>
            <div class="flex items-center gap-1.5 mt-1 text-[11px] text-muted-foreground">
              <Fingerprint class="size-3" :stroke-width="1.75" />
              <span class="font-mono truncate">{{ host.fingerprint }}</span>
            </div>
          </div>
          <button
            class="flex h-7 w-7 items-center justify-center rounded text-muted-foreground opacity-0 group-hover:opacity-100 hover:bg-destructive/20 hover:text-destructive transition-all duration-100"
            aria-label="Remove known host"
            @click="remove(host)"
          >
            <Trash2 class="size-3.5" :stroke-width="1.75" />
          </button>
        </div>
      </div>

      <!-- Empty state -->
      <div
        v-else-if="!changes.length && !removedHosts.length"
        class="flex flex-col items-center justify-center py-16 px-6 gap-3 text-center"
      >
        <ShieldCheck class="size-8 text-muted-foreground/50" :stroke-width="1.5" />
        <div>
          <p class="text-[14px] font-medium text-foreground">No known hosts</p>
          <p class="text-[12px] text-muted-foreground mt-1">
            Hosts you connect to will appear here after first use (TOFU)
          </p>
        </div>
      </div>
    </div>
  </div>
</template>
