<script setup lang="ts">
import { computed, ref, watch, nextTick, onUnmounted } from "vue";
import { useVaultStore, vaultRollbackMessage } from "../stores/vault";
import VaultRollbackNotice from "./VaultRollbackNotice.vue";
import { useUiStore } from "../stores/ui";
import Button from "./ui/Button.vue";
import Input from "./ui/Input.vue";
import FormGroup from "./ui/FormGroup.vue";
import Label from "./ui/Label.vue";
import { Vault, Unlock, Fingerprint, AlertCircle, Loader2 } from "lucide-vue-next";

const vault = useVaultStore();
const ui = useUiStore();

const passphrase = ref("");
const error = ref("");
const loading = ref<"password" | "biometric" | null>(null);
const rollback = computed(() => vaultRollbackMessage(error.value));

function clearPassphrase() {
  passphrase.value = "";
}

// This component stays mounted when its modal is hidden. Clear synchronously
// at visibility/session boundaries, including a close/reopen or lock/unlock in
// one tick, without resetting the busy guard for an in-flight authentication.
watch(() => ui.showVaultUnlockModal, clearPassphrase, { flush: "sync" });
watch(() => vault.unlocked, clearPassphrase, { flush: "sync" });
onUnmounted(clearPassphrase);

function focusPassphraseInput() {
  nextTick(() => {
    document.getElementById("modal-pass")?.focus();
  });
}

watch(
  () => ui.showVaultUnlockModal,
  async (open) => {
    if (!open) return;

    clearPassphrase();
    error.value = "";
    loading.value = null;

    // LAContext availability is runtime state, not enrollment state. Refresh it
    // whenever an SSH flow asks for the vault so a temporary Touch ID lockout at
    // app startup does not hide biometric unlock for the rest of the session.
    try {
      await vault.refreshBiometricState();
    } catch (e) {
      error.value = String(e);
    }

    if (ui.showVaultUnlockModal) {
      focusPassphraseInput();
    }
  },
);

async function submit(acceptOlder = false) {
  if (!passphrase.value || loading.value !== null) return;
  error.value = "";
  loading.value = "password";
  try {
    await vault.unlock(passphrase.value, acceptOlder);
    clearPassphrase();
    ui.resolveVaultUnlock(true);
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = null;
  }
}

async function submitBiometric() {
  if (loading.value !== null) return;
  error.value = "";
  loading.value = "biometric";
  try {
    await vault.unlockWithBiometric();
    clearPassphrase();
    ui.resolveVaultUnlock(true);
  } catch (e) {
    error.value = String(e);
    focusPassphraseInput();
  } finally {
    loading.value = null;
  }
}

function cancel() {
  // Do not resolve the caller as cancelled while a Touch ID request is still
  // capable of completing and unlocking the backend state.
  if (loading.value !== null) return;
  clearPassphrase();
  error.value = "";
  ui.resolveVaultUnlock(false);
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="ui.showVaultUnlockModal"
      class="fixed inset-0 z-[100] flex items-center justify-center bg-black/50"
      @click.self="cancel"
    >
      <div class="w-[380px] max-w-[92vw] rounded-lg border border-border bg-card p-4 sm:p-5 shadow-xl">
        <div class="flex items-center gap-3 mb-4">
          <div class="flex h-10 w-10 items-center justify-center rounded-md bg-primary/10 shrink-0">
            <Vault class="size-5 text-primary" :stroke-width="1.75" />
          </div>
          <div>
            <h3 class="text-[14px] font-semibold">Unlock Vault</h3>
            <p class="text-[12px] text-muted-foreground mt-0.5">
              The vault must be unlocked to use SSH key authentication.
            </p>
          </div>
        </div>

        <template v-if="vault.biometricAvailable && vault.biometricEnabled">
          <Button
            class="w-full"
            variant="outline"
            :disabled="loading !== null"
            @click="submitBiometric"
          >
            <Loader2
              v-if="loading === 'biometric'"
              class="size-3.5 mr-1 animate-spin"
              :stroke-width="1.75"
            />
            <Fingerprint v-else class="size-3.5 mr-1" :stroke-width="1.75" />
            {{ loading === "biometric" ? "Waiting for Touch ID..." : "Unlock with Touch ID" }}
          </Button>

          <div class="flex items-center gap-2 my-4 text-[11px] text-muted-foreground">
            <div class="flex-1 h-px bg-border"></div>
            <span>or use passphrase</span>
            <div class="flex-1 h-px bg-border"></div>
          </div>
        </template>

        <FormGroup>
          <Label for="modal-pass">Master passphrase</Label>
          <Input
            id="modal-pass"
            v-model="passphrase"
            type="password"
            placeholder="Enter master passphrase"
            :disabled="loading !== null"
            @keydown.enter="submit()"
            @keydown.escape="cancel"
          />
        </FormGroup>

        <VaultRollbackNotice v-if="rollback" class="mt-2" :message="rollback"
          :disabled="loading !== null || !passphrase" @open="submit(true)" />
        <p v-else-if="error" class="text-[12px] text-destructive flex items-center gap-1.5 mt-2">
          <AlertCircle class="size-3.5" :stroke-width="1.75" />
          {{ error }}
        </p>

        <div class="flex justify-end gap-2 mt-4">
          <Button variant="ghost" size="sm" :disabled="loading !== null" @click="cancel">
            Cancel
          </Button>
          <Button size="sm" :disabled="loading !== null || !passphrase" @click="submit()">
            <Loader2
              v-if="loading === 'password'"
              class="size-3.5 mr-1 animate-spin"
              :stroke-width="1.75"
            />
            <Unlock v-else class="size-3.5 mr-1" :stroke-width="1.75" />
            Unlock
          </Button>
        </div>
      </div>
    </div>
  </Teleport>
</template>
