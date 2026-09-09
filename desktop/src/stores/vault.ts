import { defineStore } from "pinia";
import { ref, computed } from "vue";
import * as api from "../api";
import { useKeysStore } from "./keys";

const RESET_DURABILITY_ERROR_MARKER = "[vault-reset-durability]";

export const useVaultStore = defineStore("vault", () => {
  const initialized = ref(false);
  const unlocked = ref(false);
  const error = ref<string | null>(null);
  const biometricAvailable = ref(false);
  const biometricEnabled = ref(false);
  const keysStore = useKeysStore();

  const needsSetup = computed(() => !initialized.value);
  const needsUnlock = computed(
    () => initialized.value && !unlocked.value,
  );

  let biometricStateRevision = 0;
  let biometricMutationTail: Promise<void> = Promise.resolve();

  function publishDestroyedVaultState() {
    ++biometricStateRevision;
    initialized.value = false;
    unlocked.value = false;
    biometricEnabled.value = false;
    // Key metadata is derived from the encrypted vault. Once reset crosses the
    // destructive boundary, retaining it would let host/identity forms offer
    // key IDs that no longer exist in the backend.
    keysStore.clear();
  }

  function mutateBiometricState(operation: () => Promise<void>): Promise<void> {
    // Invalidate in-flight snapshots immediately, not only after the write.
    // Serialize credential writes and destructive reset across components so
    // their backend and UI order agree.
    ++biometricStateRevision;
    const result = biometricMutationTail.then(operation);
    // A failed write must not poison the queue for subsequent operations.
    biometricMutationTail = result.catch(() => {});
    return result;
  }

  // Only used inside a serialized mutation's error recovery. Public refreshes
  // wait for that mutation, including this reconciliation, before querying.
  async function reconcileBiometricState() {
    biometricEnabled.value = false;
    if (!initialized.value) return;

    // Credential existence is independent of transient Touch ID availability.
    biometricEnabled.value = await api.biometricPassphraseStored();
  }

  async function refreshBiometricState() {
    const revision = ++biometricStateRevision;
    // A probe started during enrollment/deletion must observe the completed
    // mutation rather than read its old state and later overwrite its result.
    await biometricMutationTail;
    const isCurrent = () => revision === biometricStateRevision;
    if (!isCurrent()) return;

    try {
      const available = await api.biometricAvailable();
      if (!isCurrent()) return;
      const enabled = initialized.value
        ? await api.biometricPassphraseStored()
        : false;
      if (!isCurrent()) return;

      // Publish the snapshot together; never mix an old capability response
      // with enrollment from a newer operation.
      biometricAvailable.value = available;
      biometricEnabled.value = enabled;
    } catch (e) {
      // An obsolete failure must not erase a newer successful snapshot/write.
      if (!isCurrent()) return;
      biometricAvailable.value = false;
      biometricEnabled.value = false;
      throw e;
    }
  }

  async function checkStatus() {
    error.value = null;
    initialized.value = await api.vaultIsInitialized();
    unlocked.value = await api.isVaultUnlocked();

    try {
      // The protected Keychain item is the source of truth only for an existing
      // vault. Current LA availability is refreshed separately because it can
      // change at runtime without changing whether the credential is enrolled.
      await refreshBiometricState();
    } catch (e) {
      error.value = String(e);
    }
  }

  async function initialize(passphrase: string) {
    // Invalidate old biometric snapshots without queueing authentication behind
    // credential writes. Dispatch to the backend immediately so a later lock
    // can invalidate this initialization's authentication generation.
    ++biometricStateRevision;
    error.value = null;
    try {
      // The backend owns the entire initialization boundary: it first rejects
      // an already-initialized vault, then handles legacy Keychain cleanup and
      // creates the new vault. Do not clear biometric state before that guard,
      // or a stale frontend call could delete credentials for an existing vault.
      const unlockedAfterInitialization = await api.initializeVault(passphrase);
      // Also discard any snapshot started while initialization was pending.
      ++biometricStateRevision;
      initialized.value = true;
      unlocked.value = unlockedAfterInitialization;
      biometricEnabled.value = false;
    } catch (e) {
      error.value = String(e);
      throw e;
    }
  }

  async function unlock(passphrase: string) {
    error.value = null;
    try {
      await api.unlockVault(passphrase);
      unlocked.value = true;
    } catch (e) {
      error.value = String(e);
      throw e;
    }
  }

  async function unlockWithBiometric() {
    error.value = null;
    try {
      const success = await api.unlockWithBiometric();
      if (success) {
        unlocked.value = true;
      } else {
        throw new Error("Biometric unlock failed");
      }
    } catch (e) {
      const unlockError = e;
      try {
        // A failed attempt can itself change LocalAuthentication availability
        // (for example by entering biometric lockout), so refresh both pieces of
        // state while preserving the credential-enrollment bit independently.
        await refreshBiometricState();
      } catch {
        // refreshBiometricState already fails closed. Preserve the original
        // unlock error for the user instead of replacing it with a probe error.
      }
      error.value = String(unlockError);
      throw unlockError;
    }
  }

  async function enableBiometric(passphrase: string) {
    return mutateBiometricState(async () => {
      error.value = null;
      try {
        await api.storeBiometricPassphrase(passphrase);
        biometricEnabled.value = true;
      } catch (e) {
        const enableError = e;
        try {
          await reconcileBiometricState();
        } catch {
          // Fail closed and keep the original operation error.
        }
        error.value = String(enableError);
        throw enableError;
      }
    });
  }

  async function disableBiometric() {
    return mutateBiometricState(async () => {
      error.value = null;
      try {
        await api.clearBiometricPassphrase();
        biometricEnabled.value = false;
      } catch (e) {
        const disableError = e;
        try {
          await reconcileBiometricState();
        } catch {
          // Fail closed and keep the original operation error.
        }
        error.value = String(disableError);
        throw disableError;
      }
    });
  }

  async function lock() {
    await api.lockVault();
    unlocked.value = false;
  }

  async function reset(passphrase: string) {
    // Serialize reset behind any credential write already in flight. The backend
    // independently serializes direct IPC mutations too; this queue keeps
    // ordinary UI state deterministic.
    return mutateBiometricState(async () => {
      error.value = null;
      try {
        await api.resetVault(passphrase);
        publishDestroyedVaultState();
      } catch (e) {
        const resetError = String(e);
        if (resetError.includes(RESET_DURABILITY_ERROR_MARKER)) {
          // The backend only emits this marker after the authoritative vault file
          // has already been unlinked and its in-memory/auth state cleared. Even
          // though directory durability could not be confirmed, the renderer
          // must treat the old vault as destroyed rather than showing stale
          // unlocked state.
          publishDestroyedVaultState();
        } else {
          // Failures before the destructive boundary preserve the vault. A
          // credential may still have been removed before a later unlink error,
          // so reconcile biometric enrollment while keeping the reset-form error
          // local to the component.
          try {
            await reconcileBiometricState();
          } catch {
            // Reconciliation fails closed; preserve the original reset error.
          }
        }
        throw e;
      }
    });
  }

  return {
    initialized,
    unlocked,
    error,
    biometricAvailable,
    biometricEnabled,
    needsSetup,
    needsUnlock,
    checkStatus,
    refreshBiometricState,
    initialize,
    unlock,
    unlockWithBiometric,
    enableBiometric,
    disableBiometric,
    lock,
    reset,
  };
});
