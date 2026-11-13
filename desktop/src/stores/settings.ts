import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { APPEARANCES, applyAppearance, onSystemAppearanceChange, type Appearance } from "../lib/appearance";

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
  /** Light, dark, or whatever the operating system is set to. */
  appearance: Appearance;
}

const DEFAULTS: AppSettings = {
  autoLockMinutes: 15,
  lockOnSleep: true,
  maskAddresses: true,
  screenReaderMode: false,
  appearance: "system",
};

function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = { ...DEFAULTS, ...JSON.parse(raw) };
      if (!APPEARANCES.includes(parsed.appearance)) parsed.appearance = DEFAULTS.appearance;
      return parsed;
    }
  } catch {
    // ignore
  }
  return { ...DEFAULTS };
}

/** The saved appearance, readable before the app (and Pinia) exist. */
export function storedAppearance(): Appearance {
  return loadSettings().appearance;
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
  const appearance = ref<Appearance>(loadSettings().appearance);

  watch(
    [autoLockMinutes, lockOnSleep, maskAddresses, screenReaderMode, appearance],
    () => {
      saveSettings({
        autoLockMinutes: autoLockMinutes.value,
        lockOnSleep: lockOnSleep.value,
        maskAddresses: maskAddresses.value,
        screenReaderMode: screenReaderMode.value,
        appearance: appearance.value,
      });
    },
  );

  watch(appearance, applyAppearance, { immediate: true });
  // Following the system means following it while the app is open, too.
  // main.ts installs the same listener for screens that never create this
  // store; applying twice is harmless.
  onSystemAppearanceChange(() => {
    if (appearance.value === "system") applyAppearance("system");
  });

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

  function setAppearance(value: Appearance) {
    appearance.value = value;
  }

  return {
    autoLockMinutes,
    lockOnSleep,
    maskAddresses,
    screenReaderMode,
    appearance,
    setAutoLockMinutes,
    setLockOnSleep,
    setMaskAddresses,
    setScreenReaderMode,
    setAppearance,
  };
});
