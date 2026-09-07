<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { useHostsStore } from "../stores/hosts";
import { useKeysStore } from "../stores/keys";
import { useIdentitiesStore } from "../stores/identities";
import Dialog from "./ui/Dialog.vue";
import Button from "./ui/Button.vue";
import Input from "./ui/Input.vue";
import Select from "./ui/Select.vue";
import FormGroup from "./ui/FormGroup.vue";
import Label from "./ui/Label.vue";
import { FolderPlus } from "lucide-vue-next";
import type { Host, AuthMethod } from "../types";

const props = defineProps<{ host: Host | null }>();
const emit = defineEmits<{ close: [] }>();

const hosts = useHostsStore();
const keys = useKeysStore();
const identities = useIdentitiesStore();

const showInlineGroup = ref(false);
const inlineGroupName = ref("");

const form = ref({
  label: "",
  hostname: "",
  port: 22,
  username: "",
  useIdentity: false,
  identityId: "",
  authMode: "password" as "password" | "publickey",
  credentialKey: "",
  keyId: "",
  groupId: "",
  tags: "",
});

watch(
  () => props.host,
  (host) => {
    if (host) {
      // Agent auth is not implemented natively yet. Legacy Agent hosts open as
      // Password so saving the form cannot keep advertising an unsupported mode.
      const authMode: "password" | "publickey" =
        host.auth === "publickey" ? "publickey" : "password";
      const credentialKey =
        typeof host.auth === "object" && "password" in host.auth
          ? host.auth.password.credential_key
          : "";
      form.value = {
        label: host.label,
        hostname: host.hostname,
        port: host.port,
        username: host.username,
        useIdentity: !!host.identity_id,
        identityId: host.identity_id || "",
        authMode,
        credentialKey,
        keyId: host.key_id || "",
        groupId: host.group_id || "",
        tags: host.tags.join(", "),
      };
    } else {
      form.value = {
        label: "",
        hostname: "",
        port: 22,
        username: "",
        useIdentity: false,
        identityId: "",
        authMode: "password",
        credentialKey: "",
        keyId: "",
        groupId: "",
        tags: "",
      };
    }
  },
  { immediate: true },
);

// When an identity is selected, auto-fill the username from the identity
watch(
  () => form.value.identityId,
  (idId) => {
    if (idId && form.value.useIdentity) {
      const id = identities.identities.find((i) => i.id === idId);
      if (id && !form.value.username) {
        form.value.username = id.username;
      }
    }
  },
);

const isValid = computed(() => {
  return (
    form.value.label.trim() &&
    form.value.hostname.trim() &&
    form.value.port > 0 &&
    (form.value.useIdentity ? form.value.identityId : form.value.username.trim())
  );
});

function buildAuth(): AuthMethod {
  if (form.value.useIdentity) {
    // Auth comes from the identity, but we still store a compatibility fallback
    // on the host. A missing linked identity fails closed at connection time.
    const id = identities.identities.find((i) => i.id === form.value.identityId);
    return id?.auth ?? { password: { credential_key: "default" } };
  }
  if (form.value.authMode === "publickey") return "publickey";
  return {
    password: { credential_key: form.value.credentialKey || "default" },
  };
}

async function createInlineGroup() {
  if (!inlineGroupName.value.trim()) return;
  const group = await hosts.addGroup(inlineGroupName.value.trim());
  form.value.groupId = group.id;
  inlineGroupName.value = "";
  showInlineGroup.value = false;
}

async function save() {
  if (!isValid.value) return;
  const host: Host = {
    id: props.host?.id ?? crypto.randomUUID(),
    label: form.value.label.trim(),
    hostname: form.value.hostname.trim(),
    port: form.value.port,
    username: form.value.useIdentity
      ? identities.identities.find((i) => i.id === form.value.identityId)?.username ?? form.value.username.trim()
      : form.value.username.trim(),
    group_id: form.value.groupId || null,
    key_id: form.value.useIdentity
      ? identities.identities.find((i) => i.id === form.value.identityId)?.key_id ?? null
      : form.value.authMode === "publickey" ? form.value.keyId || null : null,
    auth: buildAuth(),
    tags: form.value.tags
      .split(",")
      .map((t) => t.trim())
      .filter(Boolean),
    startup_command: props.host?.startup_command ?? null,
    proxy_command: props.host?.proxy_command ?? null,
    jump_host_id: props.host?.jump_host_id ?? null,
    identity_id: form.value.useIdentity ? form.value.identityId || null : null,
  };
  if (props.host) {
    await hosts.updateHost(host);
  } else {
    await hosts.addHost(host);
  }
  emit("close");
}
</script>

<template>
  <Dialog
    :open="true"
    :title="host ? 'Edit Host' : 'Add Host'"
    description="Configure SSH connection details"
    width="560px"
    @close="emit('close')"
  >
    <div class="flex flex-col gap-4">
      <FormGroup>
        <Label for="host-label">Label</Label>
        <Input id="host-label" v-model="form.label" placeholder="My server" />
      </FormGroup>

      <div class="grid grid-cols-3 gap-3">
        <FormGroup class="col-span-2">
          <Label for="host-hostname">Hostname</Label>
          <Input id="host-hostname" v-model="form.hostname" placeholder="example.com or 1.2.3.4" />
        </FormGroup>
        <FormGroup>
          <Label for="host-port">Port</Label>
          <Input id="host-port" v-model.number="form.port" type="number" />
        </FormGroup>
      </div>

      <!-- Identity toggle -->
      <div class="flex gap-1 p-1 rounded-md bg-muted">
        <button
          class="flex-1 h-7 rounded text-[12px] font-medium transition-colors duration-100"
          :class="!form.useIdentity ? 'bg-background text-foreground shadow-sm' : 'text-muted-foreground'"
          @click="form.useIdentity = false"
        >
          Manual credentials
        </button>
        <button
          class="flex-1 h-7 rounded text-[12px] font-medium transition-colors duration-100"
          :class="form.useIdentity ? 'bg-background text-foreground shadow-sm' : 'text-muted-foreground'"
          @click="form.useIdentity = true"
        >
          Use identity
        </button>
      </div>

      <!-- Identity selection -->
      <template v-if="form.useIdentity">
        <FormGroup>
          <Label for="host-identity">Identity</Label>
          <Select id="host-identity" v-model="form.identityId">
            <option value="">Select an identity...</option>
            <option v-for="id in identities.identities" :key="id.id" :value="id.id">
              {{ id.label }} ({{ id.username }})
            </option>
          </Select>
          <p v-if="!identities.identities.length" class="text-[11px] text-muted-foreground mt-1">
            No identities available. Create one in the Identities section.
          </p>
        </FormGroup>
        <p v-if="form.identityId" class="text-[11px] text-muted-foreground -mt-2">
          Username, auth method, and SSH key will be resolved from the identity at connect time.
        </p>
      </template>

      <!-- Manual credentials -->
      <template v-else>
        <FormGroup>
          <Label for="host-username">Username</Label>
          <Input id="host-username" v-model="form.username" placeholder="root" />
        </FormGroup>

        <FormGroup>
          <Label>Authentication method</Label>
          <Select v-model="form.authMode">
            <option value="password">Password</option>
            <option value="publickey">SSH Key</option>
          </Select>
          <p class="text-[11px] text-muted-foreground mt-1">
            SSH Agent will return when native agent signing is implemented across supported platforms.
          </p>
        </FormGroup>

        <FormGroup v-if="form.authMode === 'password'">
          <Label for="host-cred">Credential key</Label>
          <Input id="host-cred" v-model="form.credentialKey" placeholder="Vault credential key (e.g. 'default')" />
        </FormGroup>

        <FormGroup v-else-if="form.authMode === 'publickey'">
          <Label for="host-key">SSH Key</Label>
          <Select id="host-key" v-model="form.keyId">
            <option value="">Select a key...</option>
            <option v-for="key in keys.keys" :key="key.id" :value="key.id">
              {{ key.label }} ({{ key.key_type }})
            </option>
          </Select>
          <p v-if="!keys.keys.length" class="text-[11px] text-muted-foreground mt-1">
            No keys available. Add one in the Keys section.
          </p>
        </FormGroup>
      </template>

      <!-- Group selection with inline creation -->
      <FormGroup>
        <div class="flex items-center justify-between">
          <Label for="host-group">Group (optional)</Label>
          <button
            class="inline-flex h-5 items-center gap-1 rounded text-[11px] text-muted-foreground hover:text-foreground transition-colors duration-100"
            @click="showInlineGroup = !showInlineGroup"
          >
            <FolderPlus class="size-3" :stroke-width="1.75" />
            New group
          </button>
        </div>
        <div v-if="showInlineGroup" class="flex gap-2 mt-1">
          <Input
            v-model="inlineGroupName"
            placeholder="Group name"
            @keydown.enter="createInlineGroup"
          />
          <Button size="sm" :disabled="!inlineGroupName.trim()" @click="createInlineGroup">Add</Button>
        </div>
        <Select v-else id="host-group" v-model="form.groupId">
          <option value="">No group</option>
          <option v-for="group in hosts.groups" :key="group.id" :value="group.id">
            {{ group.name }}
          </option>
        </Select>
      </FormGroup>

      <FormGroup>
        <Label for="host-tags">Tags (comma-separated)</Label>
        <Input id="host-tags" v-model="form.tags" placeholder="production, web" />
      </FormGroup>
    </div>

    <template #footer>
      <Button variant="ghost" @click="emit('close')">Cancel</Button>
      <Button :disabled="!isValid" @click="save">
        {{ host ? "Save changes" : "Add host" }}
      </Button>
    </template>
  </Dialog>
</template>
