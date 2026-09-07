import { defineStore } from "pinia";
import { ref, watch } from "vue";

const STORAGE_KEY = "clavyn-settings";
const LEGACY_STORAGE_KEY = "opentermius-settings";

export interface AppSettings {
  /** Auto-lock timeout in minutes. 0 = never. */
  autoLockMinutes: number;
  /** Lock after an extended background/timer-suspension gap is observed. */
  lockOnSleep: boolean;
}

const DEFAULTS: AppSettings = {
  autoLockMinutes: 15,
  lockOnSleep: true,
};

function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return { ...DEFAULTS, ...parsed };
    }
    // Migrate from the previous storage key so upgrading users keep their
    // auto-lock and lock-on-sleep preferences instead of silently resetting.
    const legacy = localStorage.getItem(LEGACY_STORAGE_KEY);
    if (legacy) {
      const parsed = JSON.parse(legacy);
      const migrated = { ...DEFAULTS, ...parsed };
      localStorage.setItem(STORAGE_KEY, JSON.stringify(migrated));
      localStorage.removeItem(LEGACY_STORAGE_KEY);
      return migrated;
    }
  } catch {
    // ignore
  }
  return { ...DEFAULTS };
}

function saveSettings(settings: AppSettings) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // ignore
  }
}

export const useSettingsStore = defineStore("settings", () => {
  const autoLockMinutes = ref(loadSettings().autoLockMinutes);
  const lockOnSleep = ref(loadSettings().lockOnSleep);

  watch(
    [autoLockMinutes, lockOnSleep],
    () => {
      saveSettings({
        autoLockMinutes: autoLockMinutes.value,
        lockOnSleep: lockOnSleep.value,
      });
    },
  );

  function setAutoLockMinutes(minutes: number) {
    autoLockMinutes.value = minutes;
  }

  function setLockOnSleep(enabled: boolean) {
    lockOnSleep.value = enabled;
  }

  return {
    autoLockMinutes,
    lockOnSleep,
    setAutoLockMinutes,
    setLockOnSleep,
  };
});
