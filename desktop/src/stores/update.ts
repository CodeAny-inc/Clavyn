import { defineStore } from "pinia";
import { ref, computed } from "vue";
import * as api from "../api";
import type { UnlistenFn } from "@tauri-apps/api/event";

const DISMISSED_VERSIONS_KEY = "clavyn.dismissedVersions";
const LEGACY_DISMISSED_VERSIONS_KEY = "opentermius.dismissedVersions";

function loadDismissedVersions(): string[] {
  try {
    const raw = localStorage.getItem(DISMISSED_VERSIONS_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return Array.isArray(parsed) ? parsed : [];
    }
    // Migrate from the previous key so upgrading users keep their
    // "skip this version" choices instead of being re-notified.
    const legacy = localStorage.getItem(LEGACY_DISMISSED_VERSIONS_KEY);
    if (legacy) {
      const parsed = JSON.parse(legacy);
      const versions = Array.isArray(parsed) ? parsed : [];
      localStorage.setItem(DISMISSED_VERSIONS_KEY, JSON.stringify(versions));
      localStorage.removeItem(LEGACY_DISMISSED_VERSIONS_KEY);
      return versions;
    }
  } catch {
    // ignore
  }
  return [];
}

function saveDismissedVersion(version: string) {
  const versions = loadDismissedVersions();
  if (!versions.includes(version)) {
    versions.push(version);
    localStorage.setItem(DISMISSED_VERSIONS_KEY, JSON.stringify(versions));
  }
}

function isVersionDismissed(version: string): boolean {
  return loadDismissedVersions().includes(version);
}

export const useUpdateStore = defineStore("update", () => {
  const info = ref<api.UpdateInfo | null>(null);
  const checking = ref(false);
  const installing = ref(false);
  const extracting = ref(false);
  const progress = ref(0);
  const error = ref<string | null>(null);
  const lastChecked = ref<Date | null>(null);
  const showModal = ref(false);
  const userDismissed = ref(false);

  let unlistenProgress: UnlistenFn | null = null;
  let unlistenExtracting: UnlistenFn | null = null;
  let listenersRegistered = false;

  const available = computed(() => info.value?.available === true);
  const version = computed(() => info.value?.version ?? null);
  const currentVersion = computed(() => info.value?.current_version ?? null);
  const body = computed(() => info.value?.body ?? null);
  const date = computed(() => info.value?.date ?? null);

  // True if an update is available AND the user hasn't dismissed this version
  const shouldNotify = computed(
    () =>
      available.value &&
      version.value !== null &&
      !isVersionDismissed(version.value!) &&
      !userDismissed.value,
  );

  async function registerListeners() {
    if (listenersRegistered) return;
    listenersRegistered = true;

    try {
      unlistenProgress = await api.onUpdateProgress((e) => {
        if (e.content_length && e.content_length > 0) {
          progress.value = Math.min(
            100,
            (e.chunk_length / e.content_length) * 100,
          );
        } else if (e.chunk_length > 0) {
          // Incremental progress when content_length is unknown
          progress.value = Math.min(99, progress.value + 1);
        }
      });
    } catch {
      // ignore
    }

    try {
      unlistenExtracting = await api.onUpdateExtracting(() => {
        extracting.value = true;
        progress.value = 100;
      });
    } catch {
      // ignore
    }
  }

  async function unregisterListeners() {
    unlistenProgress?.();
    unlistenExtracting?.();
    unlistenProgress = null;
    unlistenExtracting = null;
    listenersRegistered = false;
  }

  async function check() {
    if (checking.value) return;
    checking.value = true;
    error.value = null;
    try {
      const result = await api.checkForUpdates();
      info.value = result;
      lastChecked.value = new Date();
      // Reset dismissed state if a new version appeared
      if (
        result.available &&
        result.version &&
        !isVersionDismissed(result.version)
      ) {
        userDismissed.value = false;
      }
    } catch (e) {
      error.value = String(e);
      console.error("[update] check failed:", e);
    } finally {
      checking.value = false;
    }
  }

  async function install() {
    if (installing.value) return;
    installing.value = true;
    progress.value = 0;
    extracting.value = false;
    error.value = null;
    try {
      await api.installUpdate();
      // App will restart — code below may not execute
    } catch (e) {
      error.value = String(e);
      console.error("[update] install failed:", e);
      installing.value = false;
    }
  }

  function dismissForNow() {
    userDismissed.value = true;
    showModal.value = false;
  }

  function skipVersion() {
    if (version.value) {
      saveDismissedVersion(version.value);
    }
    userDismissed.value = true;
    showModal.value = false;
  }

  function showUpdateDialog() {
    if (available.value) {
      showModal.value = true;
    }
  }

  return {
    info,
    checking,
    installing,
    extracting,
    progress,
    error,
    lastChecked,
    showModal,
    available,
    version,
    currentVersion,
    body,
    date,
    shouldNotify,
    registerListeners,
    unregisterListeners,
    check,
    install,
    dismissForNow,
    skipVersion,
    showUpdateDialog,
  };
});
