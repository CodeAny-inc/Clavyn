<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { useWorkspacesStore } from "../stores/workspaces";
import { useTabsStore, isPane, type PaneTree } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { configuredSshEndpoint, effectiveSshIdentity, sshIdentityReady } from "../lib/sshIdentity";
import { useMaskedAddress } from "../composables/useMaskedAddress";
import Button from "./ui/Button.vue";
import Input from "./ui/Input.vue";
import Textarea from "./ui/Textarea.vue";
import Dialog from "./ui/Dialog.vue";
import FormGroup from "./ui/FormGroup.vue";
import Label from "./ui/Label.vue";
import Badge from "./ui/Badge.vue";
import {
  FolderOpen,
  Plus,
  Trash2,
  Pencil,
  Folder,
  Save,
  Play,
  LayoutGrid,
  Server,
  Link2,
  Zap,
  Search,
} from "lucide-vue-next";
import type { Workspace, TabLayout, PaneLayout, Host } from "../types";

const workspaces = useWorkspacesStore();
const tabs = useTabsStore();
const hosts = useHostsStore();
const identities = useIdentitiesStore();
const { maskAddress } = useMaskedAddress();

const showForm = ref(false);
const editing = ref<Workspace | null>(null);

// Form state
const form = ref({
  name: "",
  description: "",
  color: "",
  autoConnect: false,
  hostIds: [] as string[],
});

// Host picker for associating hosts
const showHostPicker = ref(false);
const pickerWorkspace = ref<Workspace | null>(null);
const hostSearchQuery = ref("");

function identityReady(host: Host) {
  return sshIdentityReady(host, identities.loaded);
}
function hostEndpoint(host: Host) {
  if (!identityReady(host)) return `Resolving SSH identity · ${host.hostname}:${host.port}`;
  return configuredSshEndpoint(host, identities.identities);
}
const filteredHostsForPicker = computed(() => {
  if (!hostSearchQuery.value.trim()) return hosts.hosts;
  const q = hostSearchQuery.value.toLowerCase();
  return hosts.hosts.filter((host) => {
    const username = identityReady(host)
      ? effectiveSshIdentity(host, identities.identities).username
      : "";
    return host.label.toLowerCase().includes(q) ||
      host.hostname.toLowerCase().includes(q) ||
      username.toLowerCase().includes(q);
  });
});

onMounted(() => {
  void workspaces.load();
  void hosts.load();
  void identities.load();
});

// --- workspace form ---
function addWorkspace() {
  editing.value = null;
  form.value = {
    name: "",
    description: "",
    color: "",
    autoConnect: false,
    hostIds: [],
  };
  showForm.value = true;
}

function editWorkspace(ws: Workspace) {
  editing.value = ws;
  form.value = {
    name: ws.name,
    description: ws.description ?? "",
    color: ws.color ?? "",
    autoConnect: ws.auto_connect ?? false,
    hostIds: [...(ws.host_ids ?? [])],
  };
  showForm.value = true;
}

async function save() {
  if (!form.value.name.trim()) return;
  const wsData: Workspace = {
    id: editing.value?.id ?? crypto.randomUUID(),
    name: form.value.name.trim(),
    tabs: editing.value?.tabs ?? [],
    icon: editing.value?.icon ?? null,
    description: form.value.description.trim() || null,
    color: form.value.color || null,
    host_ids: form.value.hostIds,
    auto_connect: form.value.autoConnect,
  };
  if (editing.value) {
    await workspaces.saveWorkspace(wsData);
  } else {
    await workspaces.createWorkspace(form.value.name.trim());
    // If we created new, also save the extra fields
    const created = workspaces.workspaces[workspaces.workspaces.length - 1];
    if (created) {
      await workspaces.saveWorkspace({
        ...created,
        description: form.value.description.trim() || null,
        color: form.value.color || null,
        host_ids: form.value.hostIds,
        auto_connect: form.value.autoConnect,
      });
    }
  }
  showForm.value = false;
}

async function remove(id: string, name: string) {
  if (confirm(`Delete workspace "${name}"?`)) {
    await workspaces.deleteWorkspace(id);
  }
}

// --- save current tab layout into a workspace ---
function paneTreeToLayout(node: PaneTree): PaneLayout {
  if (isPane(node)) {
    return {
      type: "pane",
      host_id: node.hostId ?? null,
      terminal_type: node.terminalType,
    };
  }
  return {
    type: "split",
    direction: node.direction,
    ratio: node.ratio,
    first: paneTreeToLayout(node.first),
    second: paneTreeToLayout(node.second),
  };
}

async function saveCurrentLayout(ws: Workspace) {
  const tabLayouts: TabLayout[] = tabs.tabs.map((tab) => ({
    id: tab.id,
    title: tab.title,
    layout: paneTreeToLayout(tab.tree),
  }));
  await workspaces.saveWorkspace({ ...ws, tabs: tabLayouts });
}

// --- restore a workspace layout into tabs ---
function layoutToPaneTree(layout: PaneLayout, autoConnect: boolean): PaneTree {
  if (layout.type === "pane") {
    const host: Host | undefined = layout.host_id
      ? hosts.hosts.find((h) => h.id === layout.host_id)
      : undefined;
    const terminalType = (layout.terminal_type as "ssh" | "local") ?? (host ? "ssh" : "local");
    return {
      id: crypto.randomUUID(),
      sessionId: null,
      hostId: layout.host_id ?? null,
      terminalType,
      title: host?.label ?? "Local Terminal",
      connected: false,
      closing: false,
      // Workspace policy governs SSH. Local shell panes remain useful immediately.
      autoConnect: terminalType === "local" ? true : autoConnect,
    };
  }
  return {
    id: crypto.randomUUID(),
    direction: layout.direction,
    ratio: layout.ratio,
    first: layoutToPaneTree(layout.first, autoConnect),
    second: layoutToPaneTree(layout.second, autoConnect),
  };
}

function restoreWorkspace(ws: Workspace) {
  if (!ws.tabs || ws.tabs.length === 0) {
    tabs.newTab();
    return;
  }

  // Close all existing tabs first
  const existingIds = tabs.tabs.map((t) => t.id);
  existingIds.forEach((id) => tabs.closeTab(id));

  // Recreate tabs from the workspace. auto_connect=false now means exactly that:
  // SSH xterms mount persistently but do not authenticate until the user reconnects.
  for (const tabLayout of ws.tabs) {
    const tree = layoutToPaneTree(tabLayout.layout, ws.auto_connect ?? false);
    tabs.tabs.push({
      id: tabLayout.id,
      title: tabLayout.title,
      tree,
    });
  }
  if (tabs.tabs.length > 0) {
    tabs.setActiveTab(tabs.tabs[0].id);
  }
}

// --- host association ---
function openHostPicker(ws: Workspace) {
  pickerWorkspace.value = ws;
  hostSearchQuery.value = "";
  showHostPicker.value = true;
}

function toggleHostInPicker(hostId: string) {
  if (!pickerWorkspace.value) return;
  const ids = pickerWorkspace.value.host_ids ?? [];
  if (ids.includes(hostId)) {
    pickerWorkspace.value = {
      ...pickerWorkspace.value,
      host_ids: ids.filter((id) => id !== hostId),
    };
  } else {
    pickerWorkspace.value = {
      ...pickerWorkspace.value,
      host_ids: [...ids, hostId],
    };
  }
}

async function saveHostAssociations() {
  if (!pickerWorkspace.value) return;
  await workspaces.saveWorkspace(pickerWorkspace.value);
  showHostPicker.value = false;
  pickerWorkspace.value = null;
}

function isHostAssociated(ws: Workspace, hostId: string): boolean {
  return ws.host_ids?.includes(hostId) ?? false;
}

function associatedHosts(ws: Workspace): Host[] {
  return (ws.host_ids ?? [])
    .map((id) => hosts.hosts.find((h) => h.id === id))
    .filter((h): h is Host => !!h);
}

// --- helpers ---
function paneCount(ws: Workspace): number {
  let count = 0;
  function countPanes(layout: PaneLayout) {
    if (layout.type === "pane") {
      count++;
    } else {
      countPanes(layout.first);
      countPanes(layout.second);
    }
  }
  for (const tab of ws.tabs ?? []) {
    countPanes(tab.layout);
  }
  return count;
}

const colorOptions = [
  { value: "", label: "None" },
  { value: "#3b82f6", label: "Blue" },
  { value: "#10b981", label: "Green" },
  { value: "#f59e0b", label: "Amber" },
  { value: "#ef4444", label: "Red" },
  { value: "#8b5cf6", label: "Purple" },
  { value: "#ec4899", label: "Pink" },
  { value: "#6366f1", label: "Indigo" },
];
</script>

<template>
  <div class="flex flex-col h-full overflow-hidden">
    <!-- Header -->
    <div class="flex h-11 items-center gap-2 border-b border-border px-4 pl-12 md:pl-4">
      <h2 class="text-[14px] font-semibold truncate">Workspaces</h2>
      <div class="ml-auto shrink-0">
        <Button size="sm" @click="addWorkspace">
          <Plus class="size-3.5" :stroke-width="1.75" />
          <span class="hidden sm:inline">New Workspace</span>
        </Button>
      </div>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto p-3 sm:p-4">
      <div v-if="workspaces.workspaces.length" class="flex flex-col gap-3 max-w-[800px] mx-auto">
        <div
          v-for="ws in workspaces.workspaces"
          :key="ws.id"
          class="group rounded-lg border border-border bg-card overflow-hidden transition-colors duration-100 hover:border-muted-foreground/30"
        >
          <!-- Workspace header -->
          <div class="flex items-center gap-3 p-3 sm:p-4">
            <!-- Color dot -->
            <div
              v-if="ws.color"
              class="h-3 w-3 rounded-full shrink-0"
              :style="{ backgroundColor: ws.color }"
            />
            <div class="flex h-10 w-10 items-center justify-center rounded-md bg-muted shrink-0">
              <FolderOpen class="size-5 text-muted-foreground" :stroke-width="1.75" />
            </div>
            <div class="flex-1 min-w-0">
              <div class="flex items-center gap-2 flex-wrap">
                <span class="text-[14px] font-semibold truncate">{{ ws.name }}</span>
                <Badge v-if="ws.auto_connect" class="bg-primary/10 text-primary border-primary/20">
                  <Zap class="size-2.5 mr-0.5" :stroke-width="2" />
                  Auto-connect
                </Badge>
              </div>
              <div v-if="ws.description" class="text-[12px] text-muted-foreground truncate mt-0.5">
                {{ ws.description }}
              </div>
              <div class="text-[11px] text-muted-foreground mt-0.5 flex items-center gap-2 sm:gap-3 flex-wrap">
                <span class="flex items-center gap-1">
                  <LayoutGrid class="size-3" :stroke-width="1.75" />
                  {{ ws.tabs?.length || 0 }} tab{{ (ws.tabs?.length || 0) === 1 ? '' : 's' }}
                </span>
                <span>{{ paneCount(ws) }} pane{{ paneCount(ws) === 1 ? '' : 's' }}</span>
                <span v-if="associatedHosts(ws).length" class="flex items-center gap-1">
                  <Server class="size-3" :stroke-width="1.75" />
                  {{ associatedHosts(ws).length }} host{{ associatedHosts(ws).length === 1 ? '' : 's' }}
                </span>
              </div>
            </div>
            <!-- Quick actions -->
            <div class="flex items-center gap-0.5 sm:gap-1 shrink-0">
              <button
                class="hidden sm:flex h-7 items-center gap-1 rounded px-2 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground transition-colors duration-100"
                aria-label="Save current layout"
                title="Save current tab layout into this workspace"
                @click="saveCurrentLayout(ws)"
              >
                <Save class="size-3" :stroke-width="1.75" />
                Save Layout
              </button>
              <button
                class="flex h-7 items-center gap-1 rounded px-2 text-[11px] font-medium text-primary hover:bg-primary/10 transition-colors duration-100"
                aria-label="Restore workspace"
                title="Restore this workspace's tabs"
                @click="restoreWorkspace(ws)"
              >
                <Play class="size-3" :stroke-width="1.75" />
                <span class="hidden sm:inline">Open</span>
              </button>
              <button
                class="flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground transition-colors duration-100"
                aria-label="Manage hosts"
                title="Associate hosts with this workspace"
                @click="openHostPicker(ws)"
              >
                <Link2 class="size-3.5" :stroke-width="1.75" />
              </button>
              <button
                class="flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground transition-colors duration-100"
                aria-label="Edit workspace"
                @click="editWorkspace(ws)"
              >
                <Pencil class="size-3.5" :stroke-width="1.75" />
              </button>
              <button
                class="flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:bg-destructive/20 hover:text-destructive transition-colors duration-100"
                aria-label="Delete workspace"
                @click="remove(ws.id, ws.name)"
              >
                <Trash2 class="size-3.5" :stroke-width="1.75" />
              </button>
            </div>
          </div>

          <!-- Associated hosts -->
          <div v-if="associatedHosts(ws).length" class="border-t border-border px-4 py-2.5 bg-muted/20">
            <div class="text-[10px] font-medium text-muted-foreground uppercase tracking-wide mb-1.5">
              Quick Access Hosts
            </div>
            <div class="flex flex-wrap gap-1.5">
              <div
                v-for="h in associatedHosts(ws)"
                :key="h.id"
                class="flex items-center gap-1.5 rounded-md border border-border bg-background px-2 py-1 text-[11px]"
              >
                <Server class="size-3 text-muted-foreground" :stroke-width="1.75" />
                <span class="font-medium">{{ h.label }}</span>
                <span class="text-muted-foreground font-mono">{{ maskAddress(hostEndpoint(h)) }}</span>
              </div>
            </div>
          </div>

          <!-- Empty layout hint -->
          <div
            v-if="!ws.tabs || ws.tabs.length === 0"
            class="border-t border-border px-4 py-2.5 bg-muted/20"
          >
            <p class="text-[11px] text-muted-foreground">
              No layout saved yet. Open some terminals, then click "Save Layout" to capture them.
            </p>
          </div>
        </div>
      </div>

      <!-- Empty state -->
      <div v-else class="flex flex-col items-center justify-center py-16 px-6 gap-3 text-center">
        <Folder class="size-8 text-muted-foreground/50" :stroke-width="1.5" />
        <div>
          <p class="text-[14px] font-medium text-foreground">No workspaces yet</p>
          <p class="text-[12px] text-muted-foreground mt-1 max-w-[320px]">
            Workspaces group hosts and terminal layouts by project or environment.
            Create one to organize your work.
          </p>
        </div>
        <Button size="sm" @click="addWorkspace">
          <Plus class="size-3.5" :stroke-width="1.75" />
          New Workspace
        </Button>
      </div>
    </div>

    <!-- Create/Edit workspace dialog -->
    <Dialog
      v-if="showForm"
      :open="true"
      :title="editing ? 'Edit Workspace' : 'New Workspace'"
      :description="editing ? 'Update workspace settings' : 'Create a workspace to organize hosts and terminal layouts by project'"
      width="520px"
      @close="showForm = false"
    >
      <div class="flex flex-col gap-4">
        <FormGroup>
          <Label for="ws-name">Name</Label>
          <Input id="ws-name" v-model="form.name" placeholder="Production, Staging, Dev..." @keydown.enter="save" />
        </FormGroup>

        <FormGroup>
          <Label for="ws-desc">Description (optional)</Label>
          <Textarea
            id="ws-desc"
            v-model="form.description"
            :rows="2"
            placeholder="What is this workspace for? e.g. 'Production servers and databases'"
          />
        </FormGroup>

        <FormGroup>
          <Label>Color label</Label>
          <div class="flex flex-wrap gap-2">
            <button
              v-for="opt in colorOptions"
              :key="opt.value"
              class="flex h-8 w-8 items-center justify-center rounded-md border transition-all duration-100"
              :class="form.color === opt.value ? 'border-foreground ring-2 ring-ring/20' : 'border-border hover:border-muted-foreground'"
              :style="opt.value ? { backgroundColor: opt.value } : {}"
              @click="form.color = opt.value"
            >
              <span v-if="!opt.value" class="text-[10px] text-muted-foreground">None</span>
            </button>
          </div>
        </FormGroup>

        <FormGroup>
          <div class="flex items-center justify-between">
            <div>
              <Label>Auto-connect on restore</Label>
              <p class="text-[11px] text-muted-foreground mt-0.5">
                Automatically connect to all SSH hosts when opening this workspace
              </p>
            </div>
            <button
              class="flex h-6 w-11 items-center rounded-full transition-colors duration-100 shrink-0"
              :class="form.autoConnect ? 'bg-primary' : 'bg-muted'"
              @click="form.autoConnect = !form.autoConnect"
            >
              <div
                class="h-4 w-4 rounded-full bg-white transition-transform duration-100"
                :class="form.autoConnect ? 'translate-x-6' : 'translate-x-1'"
              />
            </button>
          </div>
        </FormGroup>

        <div v-if="editing && form.hostIds.length > 0" class="text-[12px] text-muted-foreground">
          {{ form.hostIds.length }} host{{ form.hostIds.length === 1 ? '' : 's' }} associated.
          Use the link icon on the workspace card to manage hosts.
        </div>
      </div>

      <template #footer>
        <Button variant="ghost" @click="showForm = false">Cancel</Button>
        <Button :disabled="!form.name.trim()" @click="save">
          {{ editing ? "Save changes" : "Create workspace" }}
        </Button>
      </template>
    </Dialog>

    <!-- Host picker dialog -->
    <Dialog
      v-if="showHostPicker && pickerWorkspace"
      :open="true"
      title="Associate Hosts"
      :description="`Select hosts to add to '${pickerWorkspace.name}' for quick access`"
      width="520px"
      @close="showHostPicker = false"
    >
      <div class="flex flex-col gap-3">
        <!-- Search -->
        <div class="relative">
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" :stroke-width="1.75" />
          <Input
            v-model="hostSearchQuery"
            placeholder="Search hosts..."
            class="pl-8"
          />
        </div>

        <!-- Host list -->
        <div class="max-h-[300px] overflow-y-auto rounded-md border border-border">
          <div
            v-for="h in filteredHostsForPicker"
            :key="h.id"
            class="flex items-center gap-2.5 px-3 py-2 cursor-pointer transition-colors duration-100 hover:bg-muted border-b border-border last:border-b-0"
            @click="toggleHostInPicker(h.id)"
          >
            <div
              class="flex h-4 w-4 items-center justify-center rounded border transition-colors duration-100 shrink-0"
              :class="isHostAssociated(pickerWorkspace, h.id) ? 'bg-primary border-primary' : 'border-muted-foreground'"
            >
              <svg v-if="isHostAssociated(pickerWorkspace, h.id)" class="size-3 text-primary-foreground" viewBox="0 0 12 12" fill="none">
                <path d="M2 6l2.5 2.5L10 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </div>
            <Server class="size-3.5 text-muted-foreground shrink-0" :stroke-width="1.75" />
            <div class="flex-1 min-w-0">
              <span class="text-[12px] font-medium">{{ h.label }}</span>
              <span class="text-[11px] text-muted-foreground font-mono ml-2">{{ maskAddress(hostEndpoint(h)) }}</span>
            </div>
          </div>
          <div v-if="!filteredHostsForPicker.length" class="px-3 py-6 text-center text-[12px] text-muted-foreground">
            No hosts found
          </div>
        </div>

        <p class="text-[11px] text-muted-foreground">
          {{ (pickerWorkspace.host_ids ?? []).length }} host{{ (pickerWorkspace.host_ids ?? []).length === 1 ? '' : 's' }} selected
        </p>
      </div>

      <template #footer>
        <Button variant="ghost" @click="showHostPicker = false">Cancel</Button>
        <Button @click="saveHostAssociations">Save associations</Button>
      </template>
    </Dialog>
  </div>
</template>
