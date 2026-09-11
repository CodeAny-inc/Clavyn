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
  /**
   * Expose terminal output to assistive technology.
   *
   * A terminal draws its rows for the eye: xterm marks the DOM renderer's rows
   * `aria-hidden`, and the GPU renderer draws to a canvas, so neither is
   * readable by a screen reader. What is readable is xterm's accessibility
   * layer, which builds its own row elements and a live region from the buffer
   * and is independent of whichever renderer is drawing. It costs a DOM tree
   * per terminal, so it defaults to off.
   */
  screenReaderMode: boolean;
}

const DEFAULTS: AppSettings = {
  autoLockMinutes: 15,
  lockOnSleep: true,
  maskAddresses: true,
  screenReaderMode: false,
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
  const screenReaderMode = ref(loadSettings().screenReaderMode);

  watch(
    [autoLockMinutes, lockOnSleep, maskAddresses, screenReaderMode],
    () => {
      saveSettings({
        autoLockMinutes: autoLockMinutes.value,
        lockOnSleep: lockOnSleep.value,
        maskAddresses: maskAddresses.value,
        screenReaderMode: screenReaderMode.value,
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

  function setScreenReaderMode(enabled: boolean) {
    screenReaderMode.value = enabled;
  }

  return {
    autoLockMinutes,
    lockOnSleep,
    maskAddresses,
    screenReaderMode,
    setAutoLockMinutes,
    setLockOnSleep,
    setMaskAddresses,
    setScreenReaderMode,
  };
});
