import { defineStore } from "pinia";
import { ref, watch } from "vue";

const STORAGE_KEY = "clavyn-settings";

export interface AppSettings {
  /** Auto-lock timeout in minutes. 0 = never. */
  autoLockMinutes: number;
  /** Lock after an extended background/timer-suspension gap is observed. */
  lockOnSleep: boolean;
  /** Obfuscate host addresses in the UI so the full address is never shown. */
  maskAddresses: boolean;
}

const DEFAULTS: AppSettings = {
  autoLockMinutes: 15,
  lockOnSleep: true,
  maskAddresses: true,
};

function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return { ...DEFAULTS, ...parsed };
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
  const maskAddresses = ref(loadSettings().maskAddresses);

  watch(
    [autoLockMinutes, lockOnSleep, maskAddresses],
    () => {
      saveSettings({
        autoLockMinutes: autoLockMinutes.value,
        lockOnSleep: lockOnSleep.value,
        maskAddresses: maskAddresses.value,
      });
    },
  );

  function setAutoLockMinutes(minutes: number) {
    autoLockMinutes.value = minutes;
  }

  function setLockOnSleep(enabled: boolean) {
    lockOnSleep.value = enabled;
  }

  function setMaskAddresses(enabled: boolean) {
    maskAddresses.value = enabled;
  }

  return {
    autoLockMinutes,
    lockOnSleep,
    maskAddresses,
    setAutoLockMinutes,
    setLockOnSleep,
    setMaskAddresses,
  };
});
