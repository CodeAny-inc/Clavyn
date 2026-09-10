<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch } from "vue";
import { useVaultStore } from "../stores/vault";
import Button from "./ui/Button.vue";
import Input from "./ui/Input.vue";
import FormGroup from "./ui/FormGroup.vue";
import Label from "./ui/Label.vue";
import {
  Vault as VaultIcon,
  Lock,
  Unlock,
  ShieldCheck,
  AlertTriangle,
  Fingerprint,
  Loader2,
  Check,
  Trash2,
} from "lucide-vue-next";

const vault = useVaultStore();

const passphrase = ref("");
const confirmPassphrase = ref("");
const error = ref("");
const unlockLoading = ref<"password" | "biometric" | null>(null);
const enableBiometricMode = ref(false);
const biometricPassphrase = ref("");
const biometricSettingsLoading = ref<"enable" | "disable" | null>(null);
const resetMode = ref(false);
const resetPassphrase = ref("");
const resetLoading = ref(false);

const displayRuntimeError = computed(() => error.value || vault.error || "");

function clearSensitiveFormState() {
  passphrase.value = "";
  confirmPassphrase.value = "";
  resetBiometricEnableForm();
  resetResetForm();
}

function resetResetForm() {
  resetPassphrase.value = "";
  resetMode.value = false;
}

// This component stays mounted across lock/unlock, including automatic locks.
// Clear synchronously so even a lock followed by an unlock in the same tick
// cannot carry a typed secret into another authentication session.
watch(() => vault.unlocked, clearSensitiveFormState, { flush: "sync" });
onUnmounted(clearSensitiveFormState);

onMounted(async () => {
  await vault.checkStatus();
  // Enrollment and runtime availability are separate. Only auto-prompt when
  // Touch ID can currently be evaluated; an enrolled-but-temporarily-unavailable
  // credential remains visible in settings and can be disabled explicitly.
  if (
    vault.biometricAvailable &&
    vault.biometricEnabled &&
    vault.needsUnlock
  ) {
    await tryBiometricUnlock();
  }
});

async function setup() {
  error.value = "";
  if (passphrase.value !== confirmPassphrase.value) {
    error.value = "Passphrases do not match";
    return;
  }
  if (passphrase.value.length < 8) {
    error.value = "Passphrase must be at least 8 characters";
    return;
  }
  try {
    await vault.initialize(passphrase.value);
    passphrase.value = "";
    confirmPassphrase.value = "";
  } catch (e: any) {
    error.value = String(e);
  }
}

async function unlock() {
  if (!passphrase.value || unlockLoading.value !== null) return;
  error.value = "";
  unlockLoading.value = "password";
  try {
    await vault.unlock(passphrase.value);
    passphrase.value = "";
  } catch (e: any) {
    error.value = String(e);
  } finally {
    unlockLoading.value = null;
  }
}

async function lock() {
  await vault.lock();
}

async function tryBiometricUnlock() {
  if (unlockLoading.value !== null) return;
  unlockLoading.value = "biometric";
  error.value = "";
  try {
    await vault.unlockWithBiometric();
    clearSensitiveFormState();
  } catch (e: any) {
    error.value = String(e);
  } finally {
    unlockLoading.value = null;
  }
}

function resetBiometricEnableForm() {
  biometricPassphrase.value = "";
  enableBiometricMode.value = false;
}

function cancelBiometricEnable() {
  if (biometricSettingsLoading.value !== null) return;
  resetBiometricEnableForm();
  error.value = "";
}

async function enableBiometric() {
  if (!biometricPassphrase.value || biometricSettingsLoading.value !== null) return;
  error.value = "";
  biometricSettingsLoading.value = "enable";
  try {
    await vault.enableBiometric(biometricPassphrase.value);
    resetBiometricEnableForm();
  } catch (e: any) {
    // Do not keep the master passphrase referenced in component state after a
    // failed Keychain enrollment attempt. The user can explicitly enter it again.
    biometricPassphrase.value = "";
    error.value = String(e);
  } finally {
    biometricSettingsLoading.value = null;
  }
}

async function disableBiometric() {
  if (biometricSettingsLoading.value !== null) return;
  error.value = "";
  biometricSettingsLoading.value = "disable";
  try {
    await vault.disableBiometric();
  } catch (e: any) {
    error.value = String(e);
  } finally {
    biometricSettingsLoading.value = null;
  }
}

function cancelReset() {
  if (resetLoading.value) return;
  resetResetForm();
  error.value = "";
}

async function confirmReset() {
  if (!resetPassphrase.value || resetLoading.value) return;
  error.value = "";
  resetLoading.value = true;
  try {
    await vault.reset(resetPassphrase.value);
    resetResetForm();
  } catch (e: any) {
    // Do not keep the master passphrase referenced in component state after a
    // failed reset attempt. The user can explicitly enter it again.
    resetPassphrase.value = "";
    error.value = String(e);
  } finally {
    resetLoading.value = false;
  }
}

const showBiometricButton = computed(
  () => vault.biometricAvailable && vault.biometricEnabled && vault.needsUnlock,
);
</script>

<template>
  <div class="flex flex-col h-full overflow-hidden">
    <!-- Header -->
    <div class="flex h-11 items-center gap-2 border-b border-border px-4">
      <h2 class="text-[14px] font-semibold">Vault</h2>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto p-6">
      <div class="max-w-[480px] mx-auto">
        <!-- Status card -->
        <div class="flex items-center gap-3 rounded-lg border border-border bg-card p-4 mb-6">
          <div
            class="flex h-10 w-10 items-center justify-center rounded-md"
            :class="vault.unlocked ? 'bg-green-500/10' : 'bg-muted'"
          >
            <ShieldCheck
              v-if="vault.unlocked"
              class="size-5 text-green-500"
              :stroke-width="1.75"
            />
            <Lock v-else class="size-5 text-muted-foreground" :stroke-width="1.75" />
          </div>
          <div class="flex-1">
            <div class="text-[14px] font-medium">
              {{ vault.unlocked ? "Vault unlocked" : "Vault locked" }}
            </div>
            <div class="text-[12px] text-muted-foreground">
              {{ vault.initialized ? "Encrypted storage is active" : "No vault configured" }}
            </div>
          </div>
          <Button v-if="vault.unlocked" variant="outline" size="sm" @click="lock">
            <Lock class="size-3.5" :stroke-width="1.75" />
            Lock
          </Button>
        </div>

        <!-- Setup mode -->
        <div v-if="vault.needsSetup" class="flex flex-col gap-4">
          <div class="flex items-start gap-2.5 rounded-md border border-border bg-muted/50 p-3">
            <AlertTriangle class="size-4 text-yellow-500 shrink-0 mt-0.5" :stroke-width="1.75" />
            <div class="text-[12px] text-muted-foreground">
              Set up a master passphrase to encrypt your SSH keys and credentials. This passphrase cannot be recovered.
            </div>
          </div>
          <FormGroup>
            <Label for="setup-pass">Master passphrase</Label>
            <Input id="setup-pass" v-model="passphrase" type="password" placeholder="At least 8 characters" />
          </FormGroup>
          <FormGroup>
            <Label for="setup-confirm">Confirm passphrase</Label>
            <Input id="setup-confirm" v-model="confirmPassphrase" type="password" placeholder="Repeat passphrase" />
          </FormGroup>
          <p v-if="error" class="text-[12px] text-destructive">{{ error }}</p>
          <Button @click="setup">
            <VaultIcon class="size-3.5" :stroke-width="1.75" />
            Create Vault
          </Button>
        </div>

        <!-- Unlock mode -->
        <div v-else-if="vault.needsUnlock" class="flex flex-col gap-4">
          <!-- Biometric unlock button -->
          <Button
            v-if="showBiometricButton"
            variant="outline"
            :disabled="unlockLoading !== null"
            @click="tryBiometricUnlock"
          >
            <Loader2
              v-if="unlockLoading === 'biometric'"
              class="size-3.5 mr-1 animate-spin"
              :stroke-width="1.75"
            />
            <Fingerprint v-else class="size-3.5" :stroke-width="1.75" />
            {{ unlockLoading === "biometric" ? "Waiting for Touch ID..." : "Unlock with Touch ID" }}
          </Button>

          <div v-if="showBiometricButton" class="flex items-center gap-2 text-[11px] text-muted-foreground">
            <div class="flex-1 h-px bg-border"></div>
            <span>or use passphrase</span>
            <div class="flex-1 h-px bg-border"></div>
          </div>

          <FormGroup>
            <Label for="unlock-pass">Master passphrase</Label>
            <Input
              id="unlock-pass"
              v-model="passphrase"
              type="password"
              placeholder="Enter master passphrase"
              :disabled="unlockLoading !== null"
              @keydown.enter="unlock"
            />
          </FormGroup>
          <p v-if="displayRuntimeError" class="text-[12px] text-destructive">{{ displayRuntimeError }}</p>
          <Button :disabled="unlockLoading !== null || !passphrase" @click="unlock">
            <Loader2
              v-if="unlockLoading === 'password'"
              class="size-3.5 mr-1 animate-spin"
              :stroke-width="1.75"
            />
            <Unlock v-else class="size-3.5" :stroke-width="1.75" />
            Unlock
          </Button>
        </div>

        <!-- Unlocked mode -->
        <div v-else class="flex flex-col gap-4">
          <p class="text-[13px] text-muted-foreground">
            Your vault is unlocked. SSH keys and credentials are accessible.
            Lock the vault when you're done to keep your data secure.
          </p>

          <!-- Reconciliation fails closed by hiding biometric settings. Keep
               its error visible independently of those capability flags. -->
          <p v-if="displayRuntimeError" role="alert" class="text-[12px] text-destructive">
            {{ displayRuntimeError }}
          </p>

          <!-- Biometric settings -->
          <div
            v-if="vault.biometricAvailable || vault.biometricEnabled"
            class="rounded-lg border border-border p-4"
          >
            <div class="flex items-center gap-2 mb-2">
              <Fingerprint class="size-4 text-muted-foreground" :stroke-width="1.75" />
              <span class="text-[13px] font-medium">Touch ID / Biometric Unlock</span>
            </div>

            <!-- Biometric enabled -->
            <template v-if="vault.biometricEnabled">
              <p class="text-[12px] text-muted-foreground mb-3">
                <Check class="inline size-3 text-green-500 mr-1" :stroke-width="2" />
                <template v-if="vault.biometricAvailable">
                  Biometric unlock is enabled. You can unlock the vault with Touch ID
                  instead of typing your passphrase.
                </template>
                <template v-else>
                  Biometric unlock is enabled, but Touch ID is currently unavailable.
                  You can still disable biometric unlock below.
                </template>
              </p>
              <Button
                variant="outline"
                size="sm"
                :disabled="biometricSettingsLoading !== null"
                @click="disableBiometric"
              >
                <Loader2
                  v-if="biometricSettingsLoading === 'disable'"
                  class="size-3.5 mr-1 animate-spin"
                  :stroke-width="1.75"
                />
                Disable biometric unlock
              </Button>
            </template>

            <!-- Biometric not yet enabled -->
            <template v-else>
              <p class="text-[12px] text-muted-foreground mb-3">
                Enable biometric unlock to use Touch ID instead of typing your
                passphrase every time. The passphrase is stored in the OS keychain
                with biometric protection.
              </p>

              <!-- Enable form -->
              <div v-if="enableBiometricMode" class="flex flex-col gap-3">
                <FormGroup>
                  <Label for="bio-pass">Confirm master passphrase</Label>
                  <Input
                    id="bio-pass"
                    v-model="biometricPassphrase"
                    type="password"
                    placeholder="Enter your master passphrase"
                    :disabled="biometricSettingsLoading !== null"
                    @keydown.enter="enableBiometric"
                  />
                </FormGroup>
                <div class="flex items-center gap-2">
                  <Button
                    size="sm"
                    :disabled="biometricSettingsLoading !== null || !biometricPassphrase"
                    @click="enableBiometric"
                  >
                    <Loader2
                      v-if="biometricSettingsLoading === 'enable'"
                      class="size-3.5 mr-1 animate-spin"
                      :stroke-width="1.75"
                    />
                    <Fingerprint v-else class="size-3.5" :stroke-width="1.75" />
                    {{ biometricSettingsLoading === "enable" ? "Enabling..." : "Enable" }}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    :disabled="biometricSettingsLoading !== null"
                    @click="cancelBiometricEnable"
                  >
                    Cancel
                  </Button>
                </div>
              </div>

              <!-- Enable button -->
              <Button
                v-else
                variant="outline"
                size="sm"
                :disabled="biometricSettingsLoading !== null"
                @click="enableBiometricMode = true"
              >
                <Fingerprint class="size-3.5" :stroke-width="1.75" />
                Enable biometric unlock
              </Button>
            </template>
          </div>

          <Button variant="outline" @click="lock">
            <Lock class="size-3.5" :stroke-width="1.75" />
            Lock Vault Now
          </Button>

          <!-- Danger Zone -->
          <div class="rounded-lg border border-destructive/30 p-4 mt-2">
            <div class="flex items-center gap-2 mb-2">
              <AlertTriangle class="size-4 text-destructive" :stroke-width="1.75" />
              <span class="text-[13px] font-medium text-destructive">Danger Zone</span>
            </div>

            <!-- Reset confirmation form -->
            <div v-if="resetMode" class="flex flex-col gap-3">
              <div class="flex items-start gap-2.5 rounded-md border border-destructive/30 bg-destructive/5 p-3">
                <AlertTriangle class="size-4 text-destructive shrink-0 mt-0.5" :stroke-width="1.75" />
                <div class="text-[12px] text-muted-foreground">
                  Resetting the vault <strong class="text-destructive">permanently deletes all stored SSH keys and credentials</strong>.
                  This cannot be undone. Enter your master passphrase to confirm.
                </div>
              </div>
              <FormGroup>
                <Label for="reset-pass">Master passphrase</Label>
                <Input
                  id="reset-pass"
                  v-model="resetPassphrase"
                  type="password"
                  placeholder="Enter master passphrase to confirm"
                  :disabled="resetLoading"
                  @keydown.enter="confirmReset"
                />
              </FormGroup>
              <p v-if="error" class="text-[12px] text-destructive">{{ error }}</p>
              <div class="flex items-center gap-2">
                <Button
                  variant="destructive"
                  size="sm"
                  :disabled="resetLoading || !resetPassphrase"
                  @click="confirmReset"
                >
                  <Loader2
                    v-if="resetLoading"
                    class="size-3.5 mr-1 animate-spin"
                    :stroke-width="1.75"
                  />
                  <Trash2 v-else class="size-3.5" :stroke-width="1.75" />
                  {{ resetLoading ? "Resetting..." : "Reset Vault" }}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  :disabled="resetLoading"
                  @click="cancelReset"
                >
                  Cancel
                </Button>
              </div>
            </div>

            <!-- Reset button -->
            <template v-else>
              <p class="text-[12px] text-muted-foreground mb-3">
                Reset the vault to destroy all keys and credentials and start
                fresh with a new passphrase.
              </p>
              <Button
                variant="outline"
                size="sm"
                class="text-destructive border-destructive/30 hover:bg-destructive/5"
                @click="resetMode = true"
              >
                <Trash2 class="size-3.5" :stroke-width="1.75" />
                Reset Vault
              </Button>
            </template>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
