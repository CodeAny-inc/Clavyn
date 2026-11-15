<script setup lang="ts">
import { ref, computed, watch, nextTick, useId } from "vue";
import {
  Server,
  Terminal,
  KeyRound,
  FolderOpen,
  ShieldCheck,
  Vault,
  Search,
  Plus,
  UserCircle,
  Settings,
  RefreshCw,
  ArrowUpCircle,
  HardDrive,
  Lock,
} from "lucide-vue-next";
import { cn } from "../lib/cn";
import { useTabsStore } from "../stores/tabs";
import { useUpdateStore } from "../stores/update";
import { useHostsStore } from "../stores/hosts";
import { useVaultStore } from "../stores/vault";
import { useMaskedAddress } from "../composables/useMaskedAddress";
import type { Host } from "../types";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{
  close: [];
  navigate: [string];
}>();

const tabs = useTabsStore();
const update = useUpdateStore();
const hosts = useHostsStore();
const vault = useVaultStore();
const { maskAddress } = useMaskedAddress();
const listId = useId();
const query = ref("");
// Set when locking the vault fails. The palette stays open so the failure is
// visible and "Lock Vault" can be run again.
const lockError = ref<string | null>(null);
const inputRef = ref<HTMLInputElement | null>(null);

interface Command {
  id: string;
  label: string;
  icon: unknown;
  group: string;
  /** Extra text a search matches but the row does not show. */
  keywords?: string;
  detail?: string;
  host?: Host;
}

const baseCommands = computed(() => {
  const cmds: Command[] = [
    { id: "hosts", label: "Go to Hosts", icon: Server, group: "Navigation" },
    { id: "identities", label: "Go to Identities", icon: UserCircle, group: "Navigation" },
    { id: "terminal", label: "Go to Terminal", icon: Terminal, group: "Navigation" },
    { id: "files", label: "Go to Files", icon: HardDrive, group: "Navigation", keywords: "sftp" },
    { id: "keys", label: "Go to Keys", icon: KeyRound, group: "Navigation" },
    { id: "workspaces", label: "Go to Workspaces", icon: FolderOpen, group: "Navigation" },
    { id: "known-hosts", label: "Go to Known Hosts", icon: ShieldCheck, group: "Navigation" },
    { id: "vault", label: "Go to Vault Settings", icon: Vault, group: "Navigation" },
    { id: "settings", label: "Go to Settings", icon: Settings, group: "Navigation" },
    { id: "new-terminal", label: "New Terminal Tab", icon: Plus, group: "Actions" },
    { id: "check-updates", label: "Check for Updates", icon: RefreshCw, group: "Actions" },
  ];
  if (vault.unlocked) {
    cmds.push({ id: "lock-vault", label: "Lock Vault", icon: Lock, group: "Actions" });
  }
  // Every saved host can be opened from here; the search also matches its
  // address and tags, which are not shown so a masked address stays masked.
  for (const host of hosts.hosts) {
    cmds.push({
      id: `host:${host.id}`,
      label: `Connect to ${host.label}`,
      icon: Server,
      group: "Hosts",
      keywords: [host.hostname, ...host.tags].join(" "),
      detail: maskAddress(host.hostname),
      host,
    });
  }
  if (update.available) {
    cmds.push({
      id: "show-update",
      label: `Update to v${update.version}`,
      icon: ArrowUpCircle,
      group: "Actions",
    });
  }
  return cmds;
});

const filteredCommands = computed(() => {
  if (!query.value.trim()) return baseCommands.value;
  const q = query.value.trim().toLowerCase();
  return baseCommands.value.filter((c) => `${c.label} ${c.keywords ?? ""}`.toLowerCase().includes(q));
});

// The highlight is remembered by command id and its position is derived from
// it. The results change without the query changing (hosts finish loading, the
// vault auto-locks and drops "Lock Vault"), and a stored position would then
// land on a neighbouring command or past the end of the list. When the
// highlighted command leaves the results the highlight falls back to the first.
const selectedId = ref<string | null>(null);
const selectedIndex = computed(() => {
  const index = filteredCommands.value.findIndex((c) => c.id === selectedId.value);
  return index === -1 ? 0 : index;
});

function select(index: number) {
  selectedId.value = filteredCommands.value[index]?.id ?? null;
}

// A new query is a new list, so the highlight starts over at its top.
watch(query, () => {
  selectedId.value = null;
  lockError.value = null;
});

const optionId = (index: number) => `${listId}-option-${index}`;

watch(
  () => props.open,
  (open) => {
    if (open) {
      query.value = "";
      selectedId.value = null;
      lockError.value = null;
      nextTick(() => inputRef.value?.focus());
    }
  },
);

async function lockVault() {
  lockError.value = null;
  try {
    await vault.lock();
    emit("close");
  } catch (e) {
    console.error("[command-palette] failed to lock vault", e);
    lockError.value = "Could not lock the vault. Try again.";
  }
}

function execute(cmd: Command) {
  if (cmd.host) {
    tabs.newTab(cmd.host);
    emit("navigate", "terminal");
  } else if (cmd.id === "lock-vault") {
    void lockVault();
  } else if (cmd.id === "new-terminal") {
    tabs.newTab();
    emit("navigate", "terminal");
  } else if (cmd.id === "check-updates") {
    update.check();
    emit("navigate", "settings");
  } else if (cmd.id === "show-update") {
    update.showUpdateDialog();
  } else {
    emit("navigate", cmd.id);
  }
}

function handleKeydown(e: KeyboardEvent) {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    select(Math.min(selectedIndex.value + 1, filteredCommands.value.length - 1));
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    select(Math.max(selectedIndex.value - 1, 0));
  } else if (e.key === "Enter") {
    e.preventDefault();
    const cmd = filteredCommands.value[selectedIndex.value];
    if (cmd) execute(cmd);
  }
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="props.open"
      class="fixed inset-0 z-50 bg-black/50 animate-fade-in"
      @click="emit('close')"
    >
      <div
        class="absolute left-1/2 top-[20%] -translate-x-1/2 w-[560px] max-w-[92vw] px-2 sm:px-0"
        @click.stop
      >
        <div
          class="rounded-xl border border-border bg-popover text-popover-foreground shadow-dialog animate-scale-in overflow-hidden"
        >
          <!-- Search input — integrated, no separate box -->
          <div class="flex items-center gap-2.5 px-4 h-12 border-b border-border">
            <Search class="size-4 text-muted-foreground" :stroke-width="1.75" />
            <input
              ref="inputRef"
              v-model="query"
              role="combobox"
              aria-label="Search commands and hosts"
              aria-autocomplete="list"
              :aria-expanded="true"
              :aria-controls="listId"
              :aria-activedescendant="filteredCommands.length ? optionId(selectedIndex) : undefined"
              placeholder="Search commands and hosts..."
              class="flex-1 bg-transparent text-[14px] text-foreground placeholder:text-muted-foreground outline-none"
              @keydown="handleKeydown"
            />
            <kbd class="text-[10px] text-muted-foreground rounded border border-border px-1 py-0.5">Esc</kbd>
          </div>

          <div v-if="lockError" role="alert" class="px-4 py-2 border-b border-border text-[12px] text-destructive">
            {{ lockError }}
          </div>

          <!-- Results -->
          <div :id="listId" role="listbox" aria-label="Commands" class="max-h-[360px] overflow-y-auto p-1.5">
            <div
              v-for="(cmd, i) in filteredCommands"
              :id="optionId(i)"
              :key="cmd.id"
              role="option"
              :aria-selected="i === selectedIndex"
              class="flex h-[49px] items-center gap-2.5 rounded-md px-2.5 cursor-pointer transition-colors duration-100"
              :class="cn(
                i === selectedIndex ? 'bg-muted' : 'hover:bg-muted/50',
              )"
              @click="execute(cmd)"
              @mouseenter="selectedId = cmd.id"
            >
              <component
                :is="cmd.icon"
                class="size-4 text-muted-foreground shrink-0"
                :stroke-width="1.75"
              />
              <span class="flex-1 min-w-0 truncate text-[13px] text-foreground">{{ cmd.label }}</span>
              <span v-if="cmd.detail" class="hidden sm:inline font-mono text-[11px] text-muted-foreground">{{ cmd.detail }}</span>
              <span class="text-[11px] text-muted-foreground">{{ cmd.group }}</span>
            </div>
            <div v-if="!filteredCommands.length" class="py-8 text-center text-[13px] text-muted-foreground">
              No commands found
            </div>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>
