<script setup lang="ts">
import { ref, computed, onMounted } from "vue";
import * as api from "../api";
import { useMaskedAddress } from "../composables/useMaskedAddress";
import Input from "./ui/Input.vue";
import Badge from "./ui/Badge.vue";
import {
  ShieldCheck,
  Trash2,
  Search,
  Fingerprint,
} from "lucide-vue-next";
import type { KnownHostEntry } from "../types";

const hosts = ref<KnownHostEntry[]>([]);
const search = ref("");
const loading = ref(false);
const { maskAddress } = useMaskedAddress();

onMounted(async () => {
  await load();
});

async function load() {
  loading.value = true;
  try {
    hosts.value = await api.listKnownHosts();
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
  // known_hosts entries may be "host" or "host:port" or "[host]:port"
  const m = entry.match(/^\[([^\]]+)\]:(\d+)$/);
  if (m) return [m[1], parseInt(m[2], 10)];
  const parts = entry.split(":");
  if (parts.length === 2 && /^\d+$/.test(parts[1])) {
    return [parts[0], parseInt(parts[1], 10)];
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
      <div v-else class="flex flex-col items-center justify-center py-16 px-6 gap-3 text-center">
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
