<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { basename } from "@tauri-apps/api/path";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  ArrowUp,
  ChevronRight,
  Download,
  File as FileIcon,
  FileText,
  Folder,
  FolderPlus,
  HardDrive,
  Home,
  Loader2,
  Pencil,
  RefreshCw,
  Trash2,
  Upload,
} from "lucide-vue-next";
import type { SftpEntry } from "../api";
import { useHostsStore } from "../stores/hosts";
import { useIdentitiesStore } from "../stores/identities";
import { useSftpStore } from "../stores/sftp";
import {
  configuredSshEndpoint,
  effectiveSshIdentity,
  passwordAuth,
  resolvedSshHost,
  sshConfigurationKey,
  sshIdentityReady,
} from "../lib/sshIdentity";
import Button from "./ui/Button.vue";
import Input from "./ui/Input.vue";

const props = withDefaults(defineProps<{ visible?: boolean }>(), { visible: true });
const sftp = useSftpStore();
const hosts = useHostsStore();
const identities = useIdentitiesStore();

const selectedHostId = ref("");
const password = ref("");
const showConnectForm = ref(true);
const showNewDirDialog = ref(false);
const showRenameDialog = ref(false);
const newDirName = ref("");
const renameValue = ref("");
const renamingEntry = ref<SftpEntry | null>(null);
const contextMenuEntry = ref<SftpEntry | null>(null);
const showContextMenu = ref(false);
const contextMenuX = ref(0);
const contextMenuY = ref(0);

const selectedHost = computed(() =>
  hosts.hosts.find((host) => host.id === selectedHostId.value) ?? null,
);
const selectedIdentityReady = computed(() => {
  const host = selectedHost.value;
  return !host || sshIdentityReady(host, identities.loaded);
});
const selectedEffectiveIdentity = computed(() => {
  const host = selectedHost.value;
  if (!host || !selectedIdentityReady.value) return null;
  return effectiveSshIdentity(host, identities.identities);
});
const selectedConfigurationKey = computed(() => {
  const host = selectedHost.value;
  if (!host || !selectedIdentityReady.value) return null;
  return sshConfigurationKey(host, identities.identities);
});
const selectedEndpoint = computed(() => {
  const host = selectedHost.value;
  if (!host || !selectedIdentityReady.value) return null;
  return configuredSshEndpoint(host, identities.identities);
});
const passwordRequired = computed(() => {
  const effective = selectedEffectiveIdentity.value;
  return !!effective && passwordAuth(effective.auth);
});

const pathSegments = computed(() =>
  sftp.currentPath.split("/").filter(Boolean),
);

async function loadIdentities() {
  try {
    await identities.ensureLoaded();
  } catch {
    // Store exposes the retryable error in the UI.
  }
}

// A password belongs to one exact host/effective identity configuration. Never
// carry it into another host, through a hidden auth field, or across identity edits.
watch(selectedHostId, () => {
  password.value = "";
  sftp.error = null;
}, { flush: "sync" });
watch(selectedConfigurationKey, (next, previous) => {
  if (next !== previous) password.value = "";
}, { flush: "sync" });

// The Files view stays mounted so active SFTP sessions/transfers survive view
// switches. Pending credentials are different: never retain an unsubmitted
// password while this view is hidden.
watch(() => props.visible, visible => {
  if (!visible) password.value = "";
}, { flush: "sync" });

onMounted(async () => {
  await Promise.all([hosts.load(), identities.load()]);
  window.addEventListener("click", onGlobalClick);
});

onUnmounted(() => {
  password.value = "";
  window.removeEventListener("click", onGlobalClick);
  void sftp.disconnect().catch(() => {});
});

async function connect() {
  let host = selectedHost.value;
  if (!host) return;

  if (host.identity_id && !identities.loaded) {
    try {
      await identities.ensureLoaded();
    } catch {
      return;
    }
    host = hosts.hosts.find((candidate) => candidate.id === selectedHostId.value) ?? null;
    if (!host) return;
  }
  if (!sshIdentityReady(host, identities.loaded)) return;

  const configurationKey = sshConfigurationKey(host, identities.identities);
  const effective = effectiveSshIdentity(host, identities.identities);
  let credential: string | null = passwordAuth(effective.auth) ? password.value : null;
  // Submission consumes the credential immediately, just like the terminal prompt.
  password.value = "";

  try {
    const latest = hosts.hosts.find((candidate) => candidate.id === host!.id);
    if (!latest || !sshIdentityReady(latest, identities.loaded) ||
        sshConfigurationKey(latest, identities.identities) !== configurationKey) {
      sftp.error = "Connection settings changed. Re-enter credentials for the updated account.";
      return;
    }
    // Freeze the exact effective identity into this attempt. Native SFTP then
    // authenticates that reviewed username/auth/key snapshot instead of
    // re-resolving a same-id identity that may have changed after preflight.
    const transportHost = resolvedSshHost(latest, identities.identities);
    const request = sftp.connect(transportHost, credential, effective.username);
    credential = null;
    await request;
    showConnectForm.value = false;
  } catch {
    // Store exposes the actionable error in the UI.
  } finally {
    credential = null;
    password.value = "";
  }
}

async function disconnect() {
  try {
    await sftp.disconnect();
  } finally {
    showConnectForm.value = true;
    password.value = "";
  }
}

function selectEntry(entry: SftpEntry) {
  sftp.selectedEntry = entry;
}

async function openEntry(entry: SftpEntry) {
  if (entry.is_dir) {
    await sftp.navigateToDir(entry.name);
  }
}

function onEntryContextmenu(event: MouseEvent, entry: SftpEntry) {
  event.preventDefault();
  contextMenuEntry.value = entry;
  sftp.selectedEntry = entry;
  showContextMenu.value = true;
  contextMenuX.value = event.clientX;
  contextMenuY.value = event.clientY;
}

function closeContextMenu() {
  showContextMenu.value = false;
  contextMenuEntry.value = null;
}

function onGlobalClick() {
  if (showContextMenu.value) closeContextMenu();
}

async function navigateToSegment(index: number) {
  const path = `/${pathSegments.value.slice(0, index + 1).join("/")}`;
  await sftp.listDir(path);
}

async function navigateHome() {
  if (!sftp.sessionId) return;
  const api = await import("../api");
  const home = await api.sftpCanonicalize(sftp.sessionId, "~");
  await sftp.listDir(home);
}

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let index = 0;
  let value = bytes;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatDate(timestamp: number | null): string {
  if (!timestamp) return "—";
  return new Date(timestamp * 1000).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function getEntryIcon(entry: SftpEntry) {
  if (entry.is_dir) return Folder;
  if (entry.is_symlink) return FileText;
  return FileIcon;
}

async function createDir() {
  const name = newDirName.value.trim();
  if (!name) return;
  try {
    await sftp.createDir(name);
    newDirName.value = "";
    showNewDirDialog.value = false;
  } catch {
    // Store exposes the error.
  }
}

function startRename(entry: SftpEntry) {
  renamingEntry.value = entry;
  renameValue.value = entry.name;
  showRenameDialog.value = true;
  closeContextMenu();
}

async function confirmRename() {
  const entry = renamingEntry.value;
  const nextName = renameValue.value.trim();
  if (!entry || !nextName || nextName === entry.name) return;

  try {
    await sftp.renameEntry(entry, nextName);
    showRenameDialog.value = false;
    renamingEntry.value = null;
    renameValue.value = "";
  } catch {
    // Store exposes the error.
  }
}

async function deleteEntry(entry: SftpEntry) {
  closeContextMenu();
  if (!confirm(`Delete "${entry.name}"?`)) return;
  try {
    await sftp.deleteEntry(entry);
  } catch {
    // Store exposes the error.
  }
}

async function downloadEntry(entry: SftpEntry) {
  closeContextMenu();
  if (entry.is_dir) return;

  try {
    const localPath = await save({ defaultPath: entry.name });
    if (!localPath) return;
    await sftp.downloadFile(entry, localPath);
  } catch {
    // Store exposes the error.
  }
}

async function upload() {
  try {
    const localPath = await open({ multiple: false, directory: false });
    if (!localPath || typeof localPath !== "string") return;

    // Use Tauri's cross-platform path implementation rather than splitting on
    // '/', which produced invalid remote names for Windows paths.
    const fileName = await basename(localPath);
    const existing = sftp.entries.find((entry) => entry.name === fileName);
    let overwrite = false;

    if (existing) {
      if (existing.is_dir) {
        sftp.error = `A directory named "${fileName}" already exists`;
        return;
      }
      overwrite = confirm(
        `"${fileName}" already exists on the remote host. Overwrite it?`,
      );
      if (!overwrite) return;
    }

    await sftp.uploadFile(fileName, localPath, overwrite);
  } catch {
    // Store exposes transfer errors. Dialog cancellation returns above.
  }
}
</script>

<template>
  <div class="flex h-full flex-col overflow-hidden">
    <div class="flex h-11 items-center gap-2 border-b border-border px-4">
      <HardDrive class="size-4 text-muted-foreground" :stroke-width="1.75" />
      <h2 class="text-[14px] font-semibold">Files</h2>
      <span
        v-if="sftp.connectedHost"
        class="ml-2 text-[12px] text-muted-foreground"
      >
        {{ sftp.connectedHost.hostname }}:{{ sftp.connectedHost.port }}
      </span>
      <div class="flex-1" />
      <Button
        v-if="!showConnectForm"
        variant="outline"
        size="sm"
        @click="disconnect"
      >
        Disconnect
      </Button>
    </div>

    <div
      v-if="showConnectForm"
      class="flex flex-1 items-center justify-center p-6"
    >
      <div class="flex w-full max-w-[400px] flex-col gap-4">
        <div class="text-center">
          <HardDrive
            class="mx-auto mb-3 size-10 text-muted-foreground"
            :stroke-width="1.5"
          />
          <h3 class="text-[16px] font-semibold">Connect to SFTP</h3>
          <p class="mt-1 text-[12px] text-muted-foreground">
            Select a host to browse its remote filesystem
          </p>
        </div>

        <div class="flex flex-col gap-3">
          <label class="text-[12px] font-medium text-muted-foreground">Host</label>
          <select
            v-model="selectedHostId"
            class="h-9 cursor-pointer rounded-md border border-border bg-background px-3 text-[13px] text-foreground outline-none transition-colors duration-100 hover:border-muted-foreground/50"
          >
            <option value="" disabled>Select a host...</option>
            <option v-for="host in hosts.hosts" :key="host.id" :value="host.id">
              {{ host.label || host.hostname }} ({{ host.hostname }}:{{ host.port }})
            </option>
          </select>

          <p v-if="selectedHost?.identity_id && identities.loading" class="text-[12px] text-muted-foreground" role="status">
            Loading linked SSH identity… Direct hosts remain available.
          </p>
          <div v-else-if="selectedHost?.identity_id && identities.loadError" class="text-[12px] text-destructive" role="alert">
            {{ identities.loadError }}
            <button class="ml-2 underline" @click="loadIdentities">Retry identities</button>
          </div>
          <p v-else-if="selectedEndpoint" class="text-[11px] font-mono text-muted-foreground">
            {{ selectedEndpoint }}
          </p>

          <template v-if="passwordRequired">
            <label class="text-[12px] font-medium text-muted-foreground">
              Password
            </label>
            <Input
              v-model="password"
              type="password"
              placeholder="Enter SSH password"
              autocomplete="off"
              @keydown.enter="connect"
            />
          </template>

          <p
            v-if="sftp.error"
            class="flex items-center gap-1.5 text-[12px] text-destructive"
          >
            <AlertCircle class="size-3.5" :stroke-width="1.75" />
            {{ sftp.error }}
          </p>

          <Button :disabled="!selectedHostId || sftp.loading || !selectedIdentityReady" @click="connect">
            <Loader2
              v-if="sftp.loading"
              class="mr-1 size-3.5 animate-spin"
              :stroke-width="1.75"
            />
            Connect
          </Button>
        </div>
      </div>
    </div>

    <div v-else class="flex flex-1 flex-col overflow-hidden">
      <div class="flex items-center gap-1 border-b border-border px-3 py-2">
        <button
          class="flex size-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          title="Go up"
          @click="sftp.goUp()"
        >
          <ArrowUp class="size-3.5" :stroke-width="1.75" />
        </button>
        <button
          class="flex size-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          title="Home"
          @click="navigateHome"
        >
          <Home class="size-3.5" :stroke-width="1.75" />
        </button>
        <button
          class="flex size-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          title="Refresh"
          @click="sftp.refresh()"
        >
          <RefreshCw
            class="size-3.5"
            :class="{ 'animate-spin': sftp.loading }"
            :stroke-width="1.75"
          />
        </button>

        <div class="ml-2 flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto">
          <button
            class="flex h-6 items-center rounded px-1.5 text-[12px] text-muted-foreground hover:bg-muted"
            @click="sftp.listDir('/')"
          >
            /
          </button>
          <template v-for="(segment, index) in pathSegments" :key="index">
            <ChevronRight
              class="size-3 shrink-0 text-muted-foreground"
              :stroke-width="1.75"
            />
            <button
              class="flex h-6 shrink-0 items-center rounded px-1.5 text-[12px] hover:bg-muted"
              :class="
                index === pathSegments.length - 1
                  ? 'font-medium text-foreground'
                  : 'text-muted-foreground'
              "
              @click="navigateToSegment(index)"
            >
              {{ segment }}
            </button>
          </template>
        </div>

        <button
          class="flex h-7 items-center gap-1 rounded px-2 text-[12px] text-muted-foreground hover:bg-muted hover:text-foreground"
          title="New folder"
          @click="showNewDirDialog = true"
        >
          <FolderPlus class="size-3.5" :stroke-width="1.75" />
        </button>
        <button
          class="flex h-7 items-center gap-1 rounded px-2 text-[12px] text-muted-foreground hover:bg-muted hover:text-foreground"
          title="Upload"
          @click="upload"
        >
          <Upload class="size-3.5" :stroke-width="1.75" />
        </button>
      </div>

      <div
        v-if="sftp.error"
        class="flex items-center gap-2 bg-destructive/10 px-3 py-2 text-[12px] text-destructive"
      >
        <AlertCircle class="size-3.5 shrink-0" :stroke-width="1.75" />
        <span class="flex-1 truncate">{{ sftp.error }}</span>
        <button class="hover:underline" @click="sftp.error = null">Dismiss</button>
      </div>

      <div class="flex-1 overflow-y-auto" @click="sftp.selectedEntry = null">
        <table class="w-full text-[13px]">
          <thead class="sticky top-0 border-b border-border bg-background">
            <tr class="text-left text-[11px] uppercase tracking-wide text-muted-foreground">
              <th class="px-3 py-2 font-medium">Name</th>
              <th class="w-[100px] px-3 py-2 text-right font-medium">Size</th>
              <th class="w-[180px] px-3 py-2 font-medium">Modified</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="entry in sftp.entries"
              :key="entry.name"
              class="cursor-pointer border-b border-border/50 transition-colors"
              :class="
                sftp.selectedEntry?.name === entry.name
                  ? 'bg-primary/10'
                  : 'hover:bg-muted/50'
              "
              @click.stop="selectEntry(entry)"
              @dblclick="openEntry(entry)"
              @contextmenu="onEntryContextmenu($event, entry)"
            >
              <td class="px-3 py-1.5">
                <div class="flex min-w-0 items-center gap-2">
                  <component
                    :is="getEntryIcon(entry)"
                    class="size-4 shrink-0"
                    :class="entry.is_dir ? 'text-primary' : 'text-muted-foreground'"
                    :stroke-width="1.75"
                  />
                  <span class="truncate">{{ entry.name }}</span>
                </div>
              </td>
              <td class="px-3 py-1.5 text-right text-[12px] tabular-nums text-muted-foreground">
                {{ entry.is_dir ? '—' : formatSize(entry.size) }}
              </td>
              <td class="px-3 py-1.5 text-[12px] text-muted-foreground">
                {{ formatDate(entry.modified) }}
              </td>
            </tr>
          </tbody>
        </table>

        <div
          v-if="sftp.entries.length === 0 && !sftp.loading"
          class="flex flex-col items-center justify-center py-16 text-muted-foreground"
        >
          <Folder class="mb-2 size-8" :stroke-width="1.5" />
          <p class="text-[13px]">This directory is empty</p>
        </div>

        <div v-if="sftp.loading" class="flex items-center justify-center py-8">
          <Loader2 class="size-5 animate-spin text-muted-foreground" />
        </div>
      </div>

      <div class="flex items-center gap-3 border-t border-border px-3 py-1.5 text-[11px] text-muted-foreground">
        <span>{{ sftp.entries.length }} items</span>
        <span v-if="sftp.selectedEntry" class="truncate">
          Selected: {{ sftp.selectedEntry.name }}
        </span>
      </div>
    </div>

    <div
      v-if="showNewDirDialog"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      @click.self="showNewDirDialog = false"
    >
      <div class="w-[320px] rounded-lg border border-border bg-card p-4 shadow-lg">
        <h3 class="mb-3 text-[14px] font-semibold">New Folder</h3>
        <Input
          v-model="newDirName"
          placeholder="Folder name"
          class="mb-3"
          @keydown.enter="createDir"
          @keydown.escape="showNewDirDialog = false"
        />
        <div class="flex justify-end gap-2">
          <Button variant="ghost" size="sm" @click="showNewDirDialog = false">
            Cancel
          </Button>
          <Button size="sm" @click="createDir">Create</Button>
        </div>
      </div>
    </div>

    <div
      v-if="showRenameDialog"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      @click.self="showRenameDialog = false"
    >
      <div class="w-[320px] rounded-lg border border-border bg-card p-4 shadow-lg">
        <h3 class="mb-3 text-[14px] font-semibold">Rename</h3>
        <Input
          v-model="renameValue"
          placeholder="New name"
          class="mb-3"
          @keydown.enter="confirmRename"
          @keydown.escape="showRenameDialog = false"
        />
        <div class="flex justify-end gap-2">
          <Button variant="ghost" size="sm" @click="showRenameDialog = false">
            Cancel
          </Button>
          <Button size="sm" @click="confirmRename">Rename</Button>
        </div>
      </div>
    </div>

    <div
      v-if="showContextMenu && contextMenuEntry"
      class="fixed z-50 min-w-[160px] rounded-md border border-border bg-popover py-1 shadow-lg"
      :style="{ left: `${contextMenuX}px`, top: `${contextMenuY}px` }"
      @click.stop
    >
      <button
        v-if="contextMenuEntry.is_dir"
        class="flex w-full items-center gap-2 px-3 py-1.5 text-[12px] hover:bg-muted"
        @click="openEntry(contextMenuEntry); closeContextMenu()"
      >
        <Folder class="size-3.5" />
        Open
      </button>
      <button
        v-else
        class="flex w-full items-center gap-2 px-3 py-1.5 text-[12px] hover:bg-muted"
        @click="downloadEntry(contextMenuEntry)"
      >
        <Download class="size-3.5" />
        Download
      </button>
      <button
        class="flex w-full items-center gap-2 px-3 py-1.5 text-[12px] hover:bg-muted"
        @click="startRename(contextMenuEntry)"
      >
        <Pencil class="size-3.5" />
        Rename
      </button>
      <div class="my-1 h-px bg-border" />
      <button
        class="flex w-full items-center gap-2 px-3 py-1.5 text-[12px] text-destructive hover:bg-destructive/10"
        @click="deleteEntry(contextMenuEntry)"
      >
        <Trash2 class="size-3.5" />
        Delete
      </button>
    </div>
  </div>
</template>