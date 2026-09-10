<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from "vue";
import { useKeysStore } from "../stores/keys";
import Button from "./ui/Button.vue";
import Dialog from "./ui/Dialog.vue";
import Input from "./ui/Input.vue";
import Textarea from "./ui/Textarea.vue";
import FormGroup from "./ui/FormGroup.vue";
import Label from "./ui/Label.vue";
import Badge from "./ui/Badge.vue";
import {
  KeyRound,
  Plus,
  Trash2,
  Copy,
  Check,
  Eye,
  EyeOff,
  FolderOpen,
  Loader2,
} from "lucide-vue-next";
import * as api from "../api";
import { open } from "@tauri-apps/plugin-dialog";
import type { KeyMeta } from "../types";

const keys = useKeysStore();

const showAdd = ref(false);
const showPublic = ref<string | null>(null);
const copiedId = ref<string | null>(null);
const importing = ref(false);

const addForm = ref({
  label: "",
  import: false,
  privateKey: "",
  passphrase: "",
});

onMounted(() => {
  keys.load();
});

// The dialog's contents are destroyed by `v-if`, but `addForm` is not: a
// private key left there is rendered straight back into the plaintext textarea
// the next time the dialog opens, with no vault authentication in between and
// no lock to clear it. Every exit from the dialog discards the key material,
// not just the successful ones. Synchronous so nothing can observe the ref
// between the close and the clear.
watch(showAdd, (open) => {
  if (!open) clearAddForm();
}, { flush: "sync" });

// Only navigating away from this view unmounts the component today. Clearing
// here means a future caller that keeps it mounted does not silently reopen
// the retention window.
onUnmounted(clearAddForm);

async function generateKey() {
  if (!addForm.value.label.trim()) return;
  try {
    await keys.generateKey(addForm.value.label);
    resetForm();
  } catch (e) {
    console.error("Failed to generate key:", e);
    alert(`Failed to generate key: ${e}`);
  }
}

async function importKey() {
  if (!addForm.value.label.trim() || !addForm.value.privateKey.trim()) return;
  try {
    await keys.importKey(
      addForm.value.label,
      addForm.value.privateKey,
      addForm.value.passphrase || null,
    );
    resetForm();
  } catch (e) {
    console.error("Failed to import key:", e);
    // A rejected key is still a key. Discard it here rather than leaving it
    // resident for whatever the user does next.
    resetForm();
    alert(`Failed to import key: ${e}`);
  }
}

function clearAddForm() {
  addForm.value.label = "";
  addForm.value.import = false;
  addForm.value.privateKey = "";
  addForm.value.passphrase = "";
}

function resetForm() {
  showAdd.value = false;
  clearAddForm();
}

async function deleteKey(key: KeyMeta) {
  if (confirm(`Delete key "${key.label}"?`)) {
    await keys.deleteKey(key.id);
  }
}

async function copyPublicKey(key: KeyMeta) {
  try {
    await navigator.clipboard.writeText(key.public_key_base64);
    copiedId.value = key.id;
    setTimeout(() => (copiedId.value = null), 1500);
  } catch (e) {
    console.error(e);
  }
}

async function browseForKeyFile() {
  importing.value = true;
  try {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [
        { name: "SSH Private Keys", extensions: ["pem", "key", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"] },
        { name: "All Files", extensions: ["*"] },
      ],
    });
    if (typeof selected === "string" && selected) {
      const content = await api.readKeyFile(selected);
      addForm.value.privateKey = content;
    }
  } catch (e) {
    console.error("Failed to read key file:", e);
    alert(`Failed to read key file: ${e}`);
  } finally {
    importing.value = false;
  }
}
</script>

<template>
  <div class="flex flex-col h-full overflow-hidden">
    <!-- Header -->
    <div class="flex h-11 items-center gap-2 border-b border-border px-4 pl-12 md:pl-4">
      <h2 class="text-[14px] font-semibold truncate">SSH Keys</h2>
      <div class="ml-auto shrink-0">
        <Button size="sm" @click="showAdd = true">
          <Plus class="size-3.5" :stroke-width="1.75" />
          <span class="hidden sm:inline">Add Key</span>
        </Button>
      </div>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto p-3">
      <div v-if="keys.keys.length" class="flex flex-col gap-1.5">
        <div
          v-for="key in keys.keys"
          :key="key.id"
          class="group flex items-center gap-3 rounded-md border border-border bg-card p-3 transition-colors duration-100 hover:border-muted-foreground/30"
        >
          <div class="flex h-9 w-9 items-center justify-center rounded-md bg-muted">
            <KeyRound class="size-4 text-muted-foreground" :stroke-width="1.75" />
          </div>
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2">
              <span class="text-[13px] font-medium truncate">{{ key.label }}</span>
              <Badge>{{ key.key_type }}</Badge>
            </div>
            <div class="text-[11px] text-muted-foreground truncate font-mono mt-0.5">
              {{ key.fingerprint }}
            </div>
          </div>
          <div class="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
            <button
              class="flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground transition-colors duration-100"
              :aria-label="showPublic === key.id ? 'Hide public key' : 'Show public key'"
              @click="showPublic = showPublic === key.id ? null : key.id"
            >
              <Eye v-if="showPublic !== key.id" class="size-3.5" :stroke-width="1.75" />
              <EyeOff v-else class="size-3.5" :stroke-width="1.75" />
            </button>
            <button
              class="flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground transition-colors duration-100"
              aria-label="Copy public key"
              @click="copyPublicKey(key)"
            >
              <Check v-if="copiedId === key.id" class="size-3.5 text-green-500" :stroke-width="1.75" />
              <Copy v-else class="size-3.5" :stroke-width="1.75" />
            </button>
            <button
              class="flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:bg-destructive/20 hover:text-destructive transition-colors duration-100"
              aria-label="Delete key"
              @click="deleteKey(key)"
            >
              <Trash2 class="size-3.5" :stroke-width="1.75" />
            </button>
          </div>
        </div>

        <!-- Public key viewer -->
        <div
          v-if="showPublic"
          class="rounded-md border border-border bg-muted/50 p-3 mt-1"
        >
          <div class="text-[11px] text-muted-foreground mb-1.5">Public key</div>
          <pre class="text-[11px] font-mono text-foreground whitespace-pre-wrap break-all">{{ keys.keys.find(k => k.id === showPublic)?.public_key_base64 || 'Unavailable' }}</pre>
        </div>
      </div>

      <!-- Empty state -->
      <div v-else class="flex flex-col items-center justify-center py-16 px-6 gap-3 text-center">
        <KeyRound class="size-8 text-muted-foreground/50" :stroke-width="1.5" />
        <div>
          <p class="text-[14px] font-medium text-foreground">No SSH keys yet</p>
          <p class="text-[12px] text-muted-foreground mt-1">Generate a new key or import an existing one</p>
        </div>
        <Button size="sm" @click="showAdd = true">
          <Plus class="size-3.5" :stroke-width="1.75" />
          Add Key
        </Button>
      </div>
    </div>

    <!-- Add key dialog -->
    <Dialog
      :open="showAdd"
      :title="addForm.import ? 'Import Key' : 'Generate Key'"
      description="Create or import an SSH key. Keys are encrypted at rest in the vault."
      width="520px"
      @close="resetForm"
    >
      <div class="flex flex-col gap-4">
        <div class="flex gap-1 p-1 rounded-md bg-muted">
          <button
            class="flex-1 h-7 rounded text-[12px] font-medium transition-colors duration-100"
            :class="!addForm.import ? 'bg-background text-foreground shadow-sm' : 'text-muted-foreground'"
            @click="addForm.import = false"
          >
            Generate
          </button>
          <button
            class="flex-1 h-7 rounded text-[12px] font-medium transition-colors duration-100"
            :class="addForm.import ? 'bg-background text-foreground shadow-sm' : 'text-muted-foreground'"
            @click="addForm.import = true"
          >
            Import
          </button>
        </div>

        <FormGroup>
          <Label for="key-label">Label</Label>
          <Input id="key-label" v-model="addForm.label" placeholder="My laptop key" />
        </FormGroup>

        <template v-if="addForm.import">
          <FormGroup>
            <div class="flex items-center justify-between">
              <Label for="key-private">Private key (OpenSSH)</Label>
              <button
                class="inline-flex h-6 items-center gap-1 rounded text-[11px] text-muted-foreground hover:text-foreground transition-colors duration-100"
                :disabled="importing"
                @click="browseForKeyFile"
              >
                <Loader2 v-if="importing" class="size-3 animate-spin" :stroke-width="1.75" />
                <FolderOpen v-else class="size-3" :stroke-width="1.75" />
                Browse...
              </button>
            </div>
            <Textarea
              id="key-private"
              v-model="addForm.privateKey"
              :rows="6"
              placeholder="-----BEGIN OPENSSH PRIVATE KEY-----... or click Browse to select a file"
              class="font-mono text-[11px]"
            />
          </FormGroup>
          <FormGroup>
            <Label for="key-passphrase">Passphrase (optional)</Label>
            <Input id="key-passphrase" v-model="addForm.passphrase" type="password" placeholder="Leave empty if no passphrase" />
          </FormGroup>
        </template>

        <p v-else class="text-[12px] text-muted-foreground">
          A new Ed25519 key pair will be generated and stored encrypted in the vault.
        </p>
      </div>

      <template #footer>
        <Button variant="ghost" @click="resetForm">Cancel</Button>
        <Button v-if="!addForm.import" :disabled="!addForm.label.trim()" @click="generateKey">
          Generate Key
        </Button>
        <Button v-else :disabled="!addForm.label.trim() || !addForm.privateKey.trim()" @click="importKey">
          Import Key
        </Button>
      </template>
    </Dialog>
  </div>
</template>
