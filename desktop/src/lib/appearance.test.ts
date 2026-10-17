import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { applyAppearance } from "./appearance";
import { useSettingsStore } from "../stores/settings";
import SettingsView from "../components/SettingsView.vue";

// A stand-in for the system's prefers-color-scheme, which a test can flip.
function systemTheme(dark: boolean) {
  const listeners = new Set<() => void>();
  const query = {
    matches: dark,
    addEventListener: (_: string, l: () => void) => listeners.add(l),
    removeEventListener: (_: string, l: () => void) => listeners.delete(l),
  };
  vi.stubGlobal("matchMedia", () => query);
  return (next: boolean) => {
    query.matches = next;
    listeners.forEach((l) => l());
  };
}

const root = () => document.documentElement;

beforeEach(() => {
  localStorage.removeItem("clavyn-settings");
  setActivePinia(createPinia());
});

afterEach(() => {
  vi.unstubAllGlobals();
  root().classList.remove("dark");
  root().style.colorScheme = "";
  document.body.innerHTML = "";
});

describe("appearance", () => {
  it("switches the palette and the native controls together", () => {
    systemTheme(false);
    applyAppearance("dark");
    expect(root().classList.contains("dark")).toBe(true);
    expect(root().style.colorScheme).toBe("dark");
    applyAppearance("light");
    expect(root().classList.contains("dark")).toBe(false);
    expect(root().style.colorScheme).toBe("light");
  });

  it("follows the system by default, including a change while the app is open", () => {
    const setSystem = systemTheme(true);
    const settings = useSettingsStore();
    expect(settings.appearance).toBe("system");
    expect(root().classList.contains("dark")).toBe(true);

    setSystem(false);
    expect(root().classList.contains("dark")).toBe(false);
  });

  it("keeps an explicit choice when the system changes, and saves it", async () => {
    const setSystem = systemTheme(false);
    const settings = useSettingsStore();
    settings.setAppearance("dark");
    await flushPromises();
    setSystem(false);
    expect(root().classList.contains("dark")).toBe(true);
    expect(JSON.parse(localStorage.getItem("clavyn-settings")!).appearance).toBe("dark");
  });

  it("falls back to the system for a saved value it does not know", () => {
    systemTheme(false);
    localStorage.setItem("clavyn-settings", JSON.stringify({ appearance: "sepia" }));
    expect(useSettingsStore().appearance).toBe("system");
  });

  it("is chosen in Settings", async () => {
    systemTheme(false);
    setActivePinia(createPinia());
    const wrapper = mount(SettingsView, { attachTo: document.body });
    await flushPromises();
    const dark = wrapper.findAll('[role="radio"]').find((b) => b.text() === "Dark")!;
    await dark.trigger("click");
    expect(dark.attributes("aria-checked")).toBe("true");
    expect(root().classList.contains("dark")).toBe(true);
    wrapper.unmount();
  });
});
