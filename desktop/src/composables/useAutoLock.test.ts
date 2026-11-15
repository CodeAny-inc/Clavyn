import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { defineComponent, h } from "vue";
import { useAutoLock } from "./useAutoLock";
import { useVaultStore } from "../stores/vault";
import { useSettingsStore } from "../stores/settings";

let view: VueWrapper;
beforeEach(() => {
  vi.useFakeTimers();
  setActivePinia(createPinia());
  document.body.innerHTML = "";
});
afterEach(() => {
  view?.unmount();
  vi.useRealTimers();
});

describe("auto-lock activity tracking", () => {
  it("counts a key whose handler stops propagation as activity", async () => {
    const settings = useSettingsStore();
    settings.autoLockMinutes = 1;
    settings.lockOnSleep = false;
    const vault = useVaultStore();
    vault.initialized = true;
    vault.unlocked = true;
    const lock = vi.spyOn(vault, "lock").mockImplementation(async () => {
      vault.unlocked = false;
    });

    view = mount(
      defineComponent({ setup() { useAutoLock(); return () => h("textarea"); } }),
      { attachTo: document.body },
    );
    // Same shape as xterm: the focused element handles the key and stops it.
    const terminalInput = view.get("textarea").element;
    terminalInput.addEventListener("keydown", event => event.stopPropagation());

    await vi.advanceTimersByTimeAsync(50_000);
    terminalInput.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
    await vi.advanceTimersByTimeAsync(50_000);
    expect(lock).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(15_000);
    expect(lock).toHaveBeenCalledTimes(1);
  });
});
