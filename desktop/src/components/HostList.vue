<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { useHostsStore } from "../stores/hosts";
import { useTabsStore } from "../stores/tabs";
import { useIdentitiesStore } from "../stores/identities";
import { configuredSshEndpoint, effectiveSshIdentity, sshIdentityReady } from "../lib/sshIdentity";
import HostForm from "./HostForm.vue";
import Button from "./ui/Button.vue";
import Input from "./ui/Input.vue";
import Dialog from "./ui/Dialog.vue";
import FormGroup from "./ui/FormGroup.vue";
import Label from "./ui/Label.vue";
import Badge from "./ui/Badge.vue";
import {
  Plus,
  Search,
  Server,
  Pencil,
  Trash2,
  Folder,
  FolderPlus,
  ChevronRight,
  Plug,
  KeyRound,
  UserCircle,
  Lock,
  Terminal as TerminalIcon,
} from "lucide-vue-next";
import type { Host } from "../types";

const emit = defineEmits<{
  "switch-view": [view: string];
}>();

const hosts = useHostsStore();
const tabs = useTabsStore();
const identities = useIdentitiesStore();

const showForm = ref(false);
const editingHost = ref<Host | null>(null);
const showGroupForm = ref(false);
const newGroupName = ref("");

// Selection state
const selectedHostId = ref<string | null>(null);
const selectedHost = computed(() =>
  hosts.hosts.find((h) => h.id === selectedHostId.value) ?? null,
);

// Search must use the account that will really authenticate, not a stale host fallback.
const filteredHosts = computed(() => {
  let result = hosts.hosts;
  if (hosts.selectedGroupId) result = result.filter((host) => host.group_id === hosts.selectedGroupId);
  const query = hosts.searchQuery.trim().toLowerCase();
  if (!query) return result;
  return result.filter((host) => {
    const username = identityReady(host)
      ? effectiveSshIdentity(host, identities.identities).username
      : "";
    return host.label.toLowerCase().includes(query) ||
      host.hostname.toLowerCase().includes(query) ||
      username.toLowerCase().includes(query) ||
      host.tags.some((tag) => tag.toLowerCase().includes(query));
  });
});

// Connect confirmation dialog
const showConnectDialog = ref(false);
const connectTarget = ref<Host | null>(null);

onMounted(() => {
  void hosts.load();
  void identities.load();
});

function identityReady(host: Host) {
  return sshIdentityReady(host, identities.loaded);
}
function hostEndpoint(host: Host) {
  if (!identityReady(host)) {
    const state = identities.loadError ? "SSH identity unavailable" : "Resolving SSH identity";
    return `${state} · ${host.hostname}:${host.port}`;
  }
  return configuredSshEndpoint(host, identities.identities);
}
function hostUsername(host: Host) {
  return identityReady(host)
    ? effectiveSshIdentity(host, identities.identities).username
    : "SSH identity unresolved";
}
async function loadIdentities() {
  try {
    await identities.ensureLoaded();
  } catch {
    // Store exposes the retryable failure below the search field.
  }
}

function onHostClick(host: Host) {
  selectedHostId.value = host.id;
}

function onHostDblClick(host: Host) {
  selectedHostId.value = host.id;
  connectDirectly(host);
}

function onHostKeydown(e: KeyboardEvent, host: Host) {
  if (e.key === "Enter") {
    e.preventDefault();
    selectedHostId.value = host.id;
    connectDirectly(host);
  } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const list = filteredHosts.value;
    const idx = list.findIndex((h) => h.id === host.id);
    const nextIdx = e.key === "ArrowDown"
      ? Math.min(list.length - 1, idx + 1)
      : Math.max(0, idx - 1);
    selectedHostId.value = list[nextIdx]?.id ?? null;
  }
}

function connectDirectly(host: Host) {
  selectedHostId.value = host.id;
  if (!identityReady(host)) {
    void loadIdentities();
    return;
  }
  tabs.newTab(host);
  emit("switch-view", "terminal");
}

function confirmConnect() {
  if (!connectTarget.value || !identityReady(connectTarget.value)) return;
  const host = connectTarget.value;
  showConnectDialog.value = false;
  connectTarget.value = null;
  tabs.newTab(host);
  // Auto-redirect to terminal view
  emit("switch-view", "terminal");
}

function cancelConnect() {
  showConnectDialog.value = false;
  connectTarget.value = null;
}

function editHost(host: Host) {
  editingHost.value = host;
  showForm.value = true;
}

function addHost() {
  editingHost.value = null;
  showForm.value = true;
}

async function deleteHost(host: Host) {
  if (confirm(`Delete host "${host.label}"?`)) {
    await hosts.deleteHost(host.id);
    if (selectedHostId.value === host.id) {
      selectedHostId.value = null;
    }
  }
}

function selectGroup(groupId: string | null) {
  hosts.selectedGroupId = groupId;
}

async function createGroup() {
  if (!newGroupName.value.trim()) return;
  await hosts.addGroup(newGroupName.value.trim());
  newGroupName.value = "";
  showGroupForm.value = false;
}

async function deleteGroup(groupId: string, name: string) {
  if (confirm(`Delete group "${name}"? Hosts in this group will be ungrouped.`)) {
    await hosts.deleteGroup(groupId);
    if (hosts.selectedGroupId === groupId) {
      hosts.selectedGroupId = null;
    }
  }
}

// Helper: get identity label for a host
function identityLabel(host: Host): string | null {
  if (!host.identity_id || !identities.loaded) return null;
  const id = identities.identities.find((i) => i.id === host.identity_id);
  return id?.label ?? null;
}

// Helper: auth method label
function authLabel(host: Host): string {
  if (host.identity_id) return "Identity";
  if (host.auth === "publickey") return "SSH Key";
  if (host.auth === "agent") return "Agent";
  return "Password";
}
</script>

<template>
  <div class="flex flex-col h-full overflow-hidden">
    <!-- Header -->
    <div class="flex h-11 items-center gap-2 border-b border-border px-4 pl-12 md:pl-4">
      <h2 class="text-[14px] font-semibold truncate">Hosts</h2>
      <div class="ml-auto flex items-center gap-1.5 sm:gap-2 shrink-0">
        <Button size="sm" variant="ghost" class="hidden sm:inline-flex" @click="showGroupForm = true">
          <FolderPlus class="size-3.5" :stroke-width="1.75" />
          <span class="hidden sm:inline">New Group</span>
        </Button>
        <Button size="sm" variant="ghost" class="sm:hidden" @click="showGroupForm = true">
          <FolderPlus class="size-3.5" :stroke-width="1.75" />
        </Button>
        <Button size="sm" @click="addHost">
          <Plus class="size-3.5" :stroke-width="1.75" />
          <span class="hidden sm:inline">Add Host</span>
        </Button>
      </div>
    </div>

    <!-- Search -->
    <div class="px-3 py-2 border-b border-border">
      <div class="relative">
        <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" :stroke-width="1.75" />
        <Input
          v-model="hosts.searchQuery"
          placeholder="Search hosts..."
          class="pl-8"
        />
      </div>
      <p v-if="identities.loading" class="mt-2 text-[11px] text-muted-foreground" role="status">
        Loading linked SSH identities… Direct hosts remain available.
      </p>
      <div v-else-if="identities.loadError" class="mt-2 text-[11px] text-destructive" role="alert">
        {{ identities.loadError }} Direct hosts remain available.
        <button class="ml-2 underline" @click="loadIdentities">Retry identities</button>
      </div>
    </div>

    <!-- Content area -->
    <div class="flex-1 overflow-y-auto">
      <!-- Groups sidebar -->
      <div class="px-2 pt-2 pb-1">
        <button
          class="flex h-8 w-full items-center gap-2 rounded-md px-2 text-[13px] transition-colors duration-100"
          :class="hosts.selectedGroupId === null ? 'bg-sidebar-accent text-sidebar-accent-foreground' : 'text-muted-foreground hover:bg-muted'"
          @click="selectGroup(null)"
        >
          <Server class="size-3.5" :stroke-width="1.75" />
          <span>All hosts</span>
          <span class="ml-auto text-[10px] text-muted-foreground">{{ hosts.hosts.length }}</span>
        </button>
        <div
          v-for="group in hosts.groups"
          :key="group.id"
          class="group flex h-8 w-full items-center gap-2 rounded-md px-2 text-[13px] transition-colors duration-100 cursor-pointer"
          :class="hosts.selectedGroupId === group.id ? 'bg-sidebar-accent text-sidebar-accent-foreground' : 'text-muted-foreground hover:bg-muted'"
          @click="selectGroup(group.id)"
        >
          <Folder class="size-3.5 shrink-0" :stroke-width="1.75" />
          <span class="truncate flex-1">{{ group.name }}</span>
          <span class="text-[10px] text-muted-foreground">{{ hosts.hosts.filter(h => h.group_id === group.id).length }}</span>
          <button
            class="flex h-5 w-5 items-center justify-center rounded text-muted-foreground hover:bg-destructive/20 hover:text-destructive transition-colors duration-100 opacity-0 group-hover:opacity-100"
            aria-label="Delete group"
            @click.stop="deleteGroup(group.id, group.name)"
          >
            <Trash2 class="size-3" :stroke-width="1.75" />
          </button>
        </div>
        <button
          class="flex h-7 w-full items-center gap-2 rounded-md px-2 text-[12px] text-muted-foreground/70 hover:text-muted-foreground transition-colors duration-100"
          @click="showGroupForm = true"
        >
          <FolderPlus class="size-3" :stroke-width="1.75" />
          <span>Add group...</span>
        </button>
      </div>

      <div class="border-t border-border/50 mx-2 my-1"></div>

      <!-- Host list -->
      <div v-if="filteredHosts.length" class="px-2 pt-1">
        <div
          v-for="host in filteredHosts"
          :key="host.id"
          class="group flex h-10 items-center gap-2 sm:gap-2.5 rounded-md px-2 cursor-pointer transition-colors duration-100"
          :class="selectedHostId === host.id ? 'bg-accent' : 'hover:bg-muted'"
          :aria-disabled="!identityReady(host) || undefined"
          tabindex="0"
          @click="onHostClick(host)"
          @dblclick="onHostDblClick(host)"
          @keydown="onHostKeydown($event, host)"
        >
          <Server
            class="size-4 shrink-0"
            :class="selectedHostId === host.id ? 'text-foreground' : 'text-muted-foreground'"
            :stroke-width="1.75"
          />
          <div class="flex-1 min-w-0">
            <div class="text-[13px] font-medium truncate">{{ host.label }}</div>
            <div class="text-[11px] text-muted-foreground truncate font-mono">
              {{ hostEndpoint(host) }}
            </div>
          </div>
          <!-- Auth method badge -->
          <Badge
            v-if="host.identity_id"
            class="opacity-60 group-hover:opacity-100 hidden sm:inline-flex"
          >
            <UserCircle class="size-2.5 mr-0.5" :stroke-width="2" />
            Identity
          </Badge>
          <!-- Action buttons -->
          <div class="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity sm:opacity-0 sm:group-hover:opacity-100 touch:opacity-100">
            <button
              class="flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-primary/20 hover:text-primary transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-40"
              :disabled="!identityReady(host)"
              aria-label="Connect"
              title="Connect (Enter)"
              @click.stop="connectDirectly(host)"
            >
              <Plug class="size-3" :stroke-width="1.75" />
            </button>
            <button
              class="flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground transition-colors duration-100 hidden sm:flex"
              aria-label="Edit host"
              title="Edit"
              @click.stop="editHost(host)"
            >
              <Pencil class="size-3" :stroke-width="1.75" />
            </button>
            <button
              class="flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-destructive/20 hover:text-destructive transition-colors duration-100 hidden sm:flex"
              aria-label="Delete host"
              title="Delete"
              @click.stop="deleteHost(host)"
            >
              <Trash2 class="size-3" :stroke-width="1.75" />
            </button>
          </div>
          <ChevronRight class="size-3.5 text-muted-foreground opacity-0 group-hover:opacity-100 transition-opacity hidden sm:block" :stroke-width="1.75" />
        </div>
      </div>

      <!-- Empty state -->
      <div v-else class="flex flex-col items-center justify-center py-16 px-6 gap-3 text-center">
        <Server class="size-8 text-muted-foreground/50" :stroke-width="1.5" />
        <div>
          <p class="text-[14px] font-medium text-foreground">No hosts yet</p>
          <p class="text-[12px] text-muted-foreground mt-1">Add your first SSH host to get started</p>
        </div>
        <Button size="sm" @click="addHost">
          <Plus class="size-3.5" :stroke-width="1.75" />
          Add Host
        </Button>
      </div>
    </div>

    <!-- Status bar -->
    <div
      v-if="selectedHost"
      class="flex h-9 items-center gap-2 border-t border-border px-3 bg-muted/30"
    >
      <Server class="size-3.5 text-muted-foreground shrink-0" :stroke-width="1.75" />
      <span class="text-[12px] text-muted-foreground truncate flex-1 min-w-0">
        <span class="text-foreground font-medium">{{ selectedHost.label }}</span>
        <span class="ml-2 font-mono hidden sm:inline">{{ hostEndpoint(selectedHost) }}</span>
      </span>
      <Button size="sm" class="shrink-0" :disabled="!identityReady(selectedHost)" @click="connectDirectly(selectedHost)">
        <Plug class="size-3.5" :stroke-width="1.75" />
        <span class="hidden sm:inline">Connect</span>
      </Button>
    </div>

    <HostForm v-if="showForm" :host="editingHost" @close="showForm = false" />

    <!-- Connect confirmation dialog -->
    <Dialog
      v-if="showConnectDialog"
      :open="true"
      title="Connect to Host"
      width="440px"
      @close="cancelConnect"
    >
      <div v-if="connectTarget" class="flex flex-col gap-4">
        <!-- Host summary card -->
        <div class="rounded-lg border border-border bg-muted/30 p-4">
          <div class="flex items-center gap-3 mb-3">
            <div class="flex h-10 w-10 items-center justify-center rounded-md bg-primary/10 shrink-0">
              <Server class="size-5 text-primary" :stroke-width="1.75" />
            </div>
            <div class="flex-1 min-w-0">
              <div class="text-[14px] font-semibold truncate">{{ connectTarget.label }}</div>
              <div class="text-[12px] text-muted-foreground font-mono mt-0.5">
                {{ hostEndpoint(connectTarget) }}
              </div>
            </div>
          </div>

          <!-- Connection details -->
          <div class="flex flex-col gap-2 pt-3 border-t border-border">
            <div class="flex items-center gap-2 text-[12px]">
              <TerminalIcon class="size-3.5 text-muted-foreground shrink-0" :stroke-width="1.75" />
              <span class="text-muted-foreground">Connection:</span>
              <span class="font-mono text-foreground">{{ connectTarget.hostname }}:{{ connectTarget.port }}</span>
            </div>
            <div class="flex items-center gap-2 text-[12px]">
              <UserCircle class="size-3.5 text-muted-foreground shrink-0" :stroke-width="1.75" />
              <span class="text-muted-foreground">User:</span>
              <span class="font-mono text-foreground">{{ hostUsername(connectTarget) }}</span>
            </div>
            <div class="flex items-center gap-2 text-[12px]">
              <Lock class="size-3.5 text-muted-foreground shrink-0" :stroke-width="1.75" />
              <span class="text-muted-foreground">Auth:</span>
              <span class="text-foreground">{{ authLabel(connectTarget) }}</span>
              <span v-if="identityLabel(connectTarget)" class="text-muted-foreground">
                ({{ identityLabel(connectTarget) }})
              </span>
            </div>
            <div v-if="connectTarget.key_id" class="flex items-center gap-2 text-[12px]">
              <KeyRound class="size-3.5 text-muted-foreground shrink-0" :stroke-width="1.75" />
              <span class="text-muted-foreground">SSH Key:</span>
              <span class="text-foreground">Configured</span>
            </div>
          </div>
        </div>

        <p class="text-[12px] text-muted-foreground -mt-1">
          A new terminal tab will open and connect to this host via SSH.
        </p>
      </div>

      <template #footer>
        <Button variant="ghost" @click="cancelConnect">Cancel</Button>
        <Button :disabled="!!connectTarget && !identityReady(connectTarget)" @click="confirmConnect">
          <Plug class="size-3.5 mr-1" :stroke-width="1.75" />
          Connect
        </Button>
      </template>
    </Dialog>

    <!-- Group creation dialog -->
    <Dialog
      v-if="showGroupForm"
      :open="true"
      title="New Group"
      description="Create a group to organize your hosts and identities"
      width="400px"
      @close="showGroupForm = false"
    >
      <div class="flex flex-col gap-4">
        <FormGroup>
          <Label for="group-name">Group name</Label>
          <Input
            id="group-name"
            v-model="newGroupName"
            placeholder="Production, Staging, Databases..."
            @keydown.enter="createGroup"
          />
        </FormGroup>
      </div>
      <template #footer>
        <Button variant="ghost" @click="showGroupForm = false">Cancel</Button>
        <Button :disabled="!newGroupName.trim()" @click="createGroup">Create group</Button>
      </template>
    </Dialog>
  </div>
</template>
