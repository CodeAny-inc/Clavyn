import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { defineComponent, h } from "vue";
import { useFocusIntent } from "./useFocusIntent";
import { useUiStore } from "../stores/ui";
import { useUpdateStore } from "../stores/update";

let view: VueWrapper;
let intent: ReturnType<typeof useFocusIntent>;
beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  view = mount(defineComponent({ setup() { intent = useFocusIntent(); return () => h("input"); } }), { attachTo: document.body });
});
afterEach(() => { view.unmount(); });
describe("focus intent tickets", () => {
  it.each(["focusin", "pointerdown", "keydown"])("invalidates a deferred ticket after %s", event => {
    const current = intent.capture();
    expect(current()).toBe(true);
    document.dispatchEvent(new Event(event, { bubbles: true }));
    expect(current()).toBe(false);
    expect(intent.capture()()).toBe(true);
  });
  it.each(["commandPaletteOpen", "showVaultUnlockModal", "mobileSidebarOpen"] as const)("blocks %s before its DOM exists", flag => {
    useUiStore()[flag] = true;
    expect(intent.blocked()).toBe(true);
    useUiStore()[flag] = false;
    expect(intent.blocked()).toBe(false);
  });
  it("blocks a shown update", () => {
    useUpdateStore().info = { available: true, version: "0.2.0", current_version: "0.1.0", date: null, body: null };
    useUpdateStore().showModal = true;
    expect(intent.blocked()).toBe(true);
  });
  it.each(['<dialog open></dialog>', '<div data-terminal-focus-blocker></div>', '<div role="menu"></div>'])("blocks input owners: %s", html => {
    document.body.insertAdjacentHTML("beforeend", html);
    expect(intent.blocked()).toBe(true);
  });
});
