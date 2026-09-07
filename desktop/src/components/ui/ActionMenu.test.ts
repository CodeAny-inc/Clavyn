import { afterEach, expect, it, vi } from "vitest";
import { mount, flushPromises, type VueWrapper } from "@vue/test-utils";
import ActionMenu from "./ActionMenu.vue";
let wrapper: VueWrapper | undefined;
afterEach(() => { wrapper?.unmount(); wrapper = undefined; });
function mountMenu() {
  wrapper = mount(ActionMenu, { attachTo: document.body, props: { label: "Pane actions", items: [
    { id: "split", label: "Split right" }, { id: "reconnect", label: "Reconnect", disabled: true }, { id: "close", label: "Close session", danger: true },
  ] } });
  return wrapper;
}
it("supports keyboard open/navigation, skips disabled actions, and restores focus with Escape", async () => {
  const menu = mountMenu();
  await menu.get("button").trigger("keydown", { key: "ArrowDown" });
  await flushPromises();
  expect(document.activeElement?.textContent).toContain("Split right");
  document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  expect(document.activeElement?.textContent).toContain("Close session");
  document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await flushPromises();
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(menu.get("button").element);
});
it("emits the selected action exactly once and dismisses", async () => {
  const menu = mountMenu(); await menu.get("button").trigger("click"); await flushPromises();
  (document.querySelector('[role="menuitem"]') as HTMLButtonElement).click(); await flushPromises();
  expect(menu.emitted("select")).toEqual([["split"]]);
  expect(document.querySelector('[role="menu"]')).toBeNull();
});
it("dismisses on outside pointer input and when its owning pane is hidden", async () => {
  const menu = mountMenu(); await menu.get("button").trigger("click"); await flushPromises();
  document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true })); await flushPromises();
  expect(document.querySelector('[role="menu"]')).toBeNull();
  await menu.get("button").trigger("click"); await flushPromises();
  await menu.setProps({ enabled: false });
  expect(document.querySelector('[role="menu"]')).toBeNull();
});
it("does not register delayed listeners after unmount during an open", async () => {
  const menu = mountMenu();
  const spy = vi.spyOn(document, "addEventListener");
  (menu.get("button").element as HTMLButtonElement).click();
  menu.unmount(); wrapper = undefined;
  await flushPromises();
  expect(spy.mock.calls.filter(([event]) => event === "pointerdown")).toHaveLength(0);
  spy.mockRestore();
});
