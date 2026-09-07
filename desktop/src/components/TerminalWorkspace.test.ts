import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import { useTabsStore } from "../stores/tabs";
import TerminalWorkspace from "./TerminalWorkspace.vue";
const lifecycle = vi.hoisted(() => ({ mounted: [] as string[], destroyed: [] as string[] }));
vi.mock("./TerminalPane.vue", async () => {
  const { defineComponent, h, onMounted, onBeforeUnmount } = await import("vue");
  return { default: defineComponent({
    props: ["pane", "tabId", "visible"],
    setup(props) {
      onMounted(() => lifecycle.mounted.push(props.pane.id));
      onBeforeUnmount(() => lifecycle.destroyed.push(props.pane.id));
      return () => h("div", { "data-terminal": props.pane.id }, props.pane.title);
    },
  }) };
});
let wrapper: VueWrapper | undefined;
beforeEach(() => {
  setActivePinia(createPinia());
  lifecycle.mounted.length = 0;
  lifecycle.destroyed.length = 0;
});
afterEach(() => { wrapper?.unmount(); wrapper = undefined; });
it("keeps the same component across splits, Files visibility, moves and tab switches", async () => {
  const store = useTabsStore();
  const original = store.newTab();
  const first = store.activePaneId!;
  wrapper = mount(TerminalWorkspace, { props: { visible: true } });
  await nextTick();
  const originalElement = wrapper.get(`[data-terminal="${first}"]`).element;
  const second = store.splitPane(first, "horizontal")!;
  await nextTick();
  store.splitPane(second.id, "vertical");
  await nextTick();
  await wrapper.setProps({ visible: false });
  await wrapper.setProps({ visible: true });
  const target = store.newTab();
  await nextTick();
  store.movePaneToTab(first, target.id);
  await nextTick();
  store.setActiveTab(original.id);
  await nextTick();
  store.setActiveTab(target.id);
  await nextTick();
  expect(wrapper.get(`[data-terminal="${first}"]`).element).toBe(originalElement);
  expect(lifecycle.mounted.filter(id => id === first)).toHaveLength(1);
  expect(lifecycle.destroyed).toEqual([]);
  store.closePane(first);
  await nextTick();
  expect(lifecycle.destroyed).toEqual([first]);
});
it("preserves both terminal elements on center-swap and edge-drop", async () => {
  const store = useTabsStore();
  store.newTab();
  const first = store.activePaneId!;
  const second = store.splitPane(first, "horizontal")!;
  wrapper = mount(TerminalWorkspace, { props: { visible: true } });
  await nextTick();
  store.startDrag(first); store.dropPane(second.id, "center");
  await nextTick();
  store.startDrag(first); store.dropPane(second.id, "bottom");
  await nextTick();
  expect(lifecycle.mounted).toHaveLength(2);
  expect(lifecycle.destroyed).toEqual([]);
});
it("allows keyboard resize and equal-size reset", async () => {
  const store = useTabsStore();
  store.newTab();
  store.splitPane(store.activePaneId!, "horizontal");
  wrapper = mount(TerminalWorkspace, { props: { visible: true } });
  const divider = wrapper.get('[role="separator"]');
  await divider.trigger("keydown", { key: "ArrowRight" });
  expect(divider.attributes("aria-valuenow")).toBe("55");
  await divider.trigger("keydown", { key: "Home" });
  expect(divider.attributes("aria-valuenow")).toBe("50");
});
