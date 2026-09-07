<script setup lang="ts">
import { Server, Terminal, KeyRound, FolderOpen, HardDrive, ShieldCheck, Vault, Search,
  Settings, UserCircle, PanelLeftClose, PanelLeftOpen, X } from "lucide-vue-next";
import { computed } from "vue";
import { useUpdateStore } from "../stores/update";
import { useUiStore } from "../stores/ui";

defineProps<{ activeView: string; vaultUnlocked: boolean; tabCount: number }>();
const emit = defineEmits<{ navigate: [string]; "open-command-palette": [] }>();
const update = useUpdateStore();
const ui = useUiStore();
const compact = computed(() => ui.sidebarCollapsed && !ui.mobileSidebarOpen);
const sections = [
  [
    { id: "hosts", label: "Hosts", icon: Server },
    { id: "terminal", label: "Terminal", icon: Terminal },
    { id: "files", label: "Files", icon: HardDrive },
    { id: "workspaces", label: "Workspaces", icon: FolderOpen },
  ],
  [
    { id: "identities", label: "Identities", icon: UserCircle },
    { id: "keys", label: "Keys", icon: KeyRound },
    { id: "known-hosts", label: "Known Hosts", icon: ShieldCheck },
  ],
];
function navigate(view: string) { emit("navigate", view); ui.closeMobileSidebar(); }
</script>

<template>
  <div v-if="ui.mobileSidebarOpen" class="fixed inset-0 z-40 bg-black/50 md:hidden" @click="ui.closeMobileSidebar()" />
  <aside class="app-sidebar" :class="[compact ? 'sidebar-collapsed' : '', ui.mobileSidebarOpen ? 'sidebar-open' : '']" aria-label="Application navigation">
    <div class="sidebar-brand">
      <span v-if="!compact" class="flex min-w-0 flex-1 items-center gap-2 text-[13px] font-semibold tracking-tight">
        <span class="flex size-6 items-center justify-center rounded-lg bg-blue-400/15 text-blue-300"><Terminal class="size-3.5" /></span>OpenTermius
      </span>
      <button class="hidden size-8 shrink-0 items-center justify-center rounded-md text-slate-400 hover:bg-white/10 hover:text-white md:flex"
        :aria-label="ui.sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'" :title="ui.sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'" @click="ui.toggleSidebar()">
        <PanelLeftOpen v-if="ui.sidebarCollapsed" class="size-4" /><PanelLeftClose v-else class="size-4" />
      </button>
      <button class="ml-auto flex size-8 items-center justify-center rounded-md hover:bg-white/10 md:hidden" aria-label="Close navigation" @click="ui.closeMobileSidebar()"><X class="size-4" /></button>
    </div>
    <div class="px-3 pb-3 pt-5">
      <button class="sidebar-search" aria-label="Search commands" title="Search commands (Cmd/Ctrl+K)" @click="emit('open-command-palette')">
        <Search class="size-4 shrink-0" :stroke-width="1.75" />
        <template v-if="!compact"><span class="flex-1 text-left">Search</span><kbd class="text-[10px] opacity-70">⌘ K</kbd></template>
      </button>
    </div>
    <nav class="min-h-0 flex-1 overflow-y-auto px-3" aria-label="Main navigation">
      <div v-for="(section, index) in sections" :key="index" :class="index ? 'mt-4 border-t border-sidebar-border pt-4' : ''">
        <button v-for="item in section" :key="item.id" class="sidebar-link" :class="{ 'sidebar-link-active': activeView === item.id }"
          :aria-label="item.label" :title="ui.sidebarCollapsed ? item.label : undefined" :aria-current="activeView === item.id ? 'page' : undefined" @click="navigate(item.id)">
          <component :is="item.icon" class="size-4 shrink-0" :stroke-width="1.75" />
          <template v-if="!compact"><span class="min-w-0 flex-1 truncate text-left">{{ item.label }}</span>
            <span v-if="item.id === 'terminal' && tabCount" class="text-[11px] text-muted-foreground">{{ tabCount }}</span></template>
        </button>
      </div>
    </nav>
    <!-- Vault appears once, with its real state; creation lives in the session strip. -->
    <div class="mx-3 border-t border-sidebar-border py-3">
      <button class="sidebar-link" :class="{ 'sidebar-link-active': activeView === 'vault' }" :aria-label="`Vault, ${vaultUnlocked ? 'unlocked' : 'locked'}`" :title="`Vault: ${vaultUnlocked ? 'unlocked' : 'locked'}`" @click="navigate('vault')">
        <span class="relative flex size-4 shrink-0 items-center justify-center">
          <Vault class="size-4" :stroke-width="1.75" />
          <span v-if="compact" data-testid="compact-vault-status" class="absolute -right-1 -top-1 size-2 rounded-full ring-2 ring-sidebar"
            :class="vaultUnlocked ? 'bg-emerald-500' : 'bg-muted-foreground'" aria-hidden="true" />
        </span>
        <template v-if="!compact"><span class="flex-1 text-left">Vault</span><span class="size-1.5 rounded-full" :class="vaultUnlocked ? 'bg-emerald-500' : 'bg-muted-foreground'" /></template>
      </button>
      <button class="sidebar-link" :class="{ 'sidebar-link-active': activeView === 'settings' }"
        :aria-label="update.available ? 'Settings, update available' : 'Settings'"
        :title="update.available ? 'Settings: update available' : 'Settings'" @click="navigate('settings')">
        <span class="relative flex size-4 shrink-0 items-center justify-center">
          <Settings class="size-4" :stroke-width="1.75" />
          <span v-if="compact && update.available" data-testid="compact-update-status"
            class="absolute -right-1 -top-1 size-2 rounded-full bg-blue-500 ring-2 ring-sidebar" aria-hidden="true" />
        </span>
        <template v-if="!compact"><span class="flex-1 text-left">Settings</span><span v-if="update.available" class="size-1.5 rounded-full bg-blue-500" title="Update available" /></template>
      </button>
    </div>
  </aside>
</template>

<style scoped>
.app-sidebar { @apply fixed inset-y-0 left-0 z-50 flex w-[216px] shrink-0 -translate-x-full flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground md:relative md:translate-x-0; }
.sidebar-open { @apply translate-x-0; }
.sidebar-collapsed { @apply md:w-[60px]; }
.sidebar-brand { @apply flex h-12 shrink-0 items-center gap-2 px-3 text-slate-100; background: var(--workspace-chrome); }
.sidebar-search { @apply flex h-9 w-full items-center gap-2 rounded-lg border border-sidebar-border px-2.5 text-xs text-muted-foreground hover:bg-sidebar-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring; }
.sidebar-link { @apply mb-1 flex h-10 w-full items-center gap-3 rounded-lg px-2.5 text-[13px] text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring; }
.sidebar-link-active { @apply bg-sidebar-accent text-sidebar-accent-foreground font-medium; }
@media (min-width: 768px) { .sidebar-collapsed .sidebar-link, .sidebar-collapsed .sidebar-search { justify-content: center; padding-left: 0; padding-right: 0; } }
</style>
