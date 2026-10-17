import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { DOMWrapper, mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import CommandPalette from "./CommandPalette.vue";

let wrapper: VueWrapper | undefined;
beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
});
afterEach(() => {
  wrapper?.unmount(); wrapper = undefined;
});

describe("CommandPalette keyboard selection", () => {
  it("runs the first match on Enter after the query narrows the list", async () => {
    wrapper = mount(CommandPalette, { props: { open: false }, attachTo: document.body });
    await wrapper.setProps({ open: true });
    await nextTick();
    const input = new DOMWrapper(document.querySelector<HTMLInputElement>('input[placeholder="Search commands..."]')!);

    for (let i = 0; i < 6; i++) await input.trigger("keydown", { key: "ArrowDown" });
    await input.setValue("host");
    await input.trigger("keydown", { key: "Enter" });

    expect(wrapper.emitted("navigate")).toEqual([["hosts"]]);
  });
});
