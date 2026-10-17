import { afterEach, describe, expect, it } from "vitest";
import { defineComponent, h, nextTick, ref } from "vue";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import Dialog from "./Dialog.vue";

let wrapper: VueWrapper | undefined;
afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  document.body.innerHTML = "";
});

// A page with a button that opens a form dialog, the way the app's views do.
function mountForm(props: Record<string, unknown> = {}) {
  const open = ref(false);
  const closes = ref(0);
  const Host = defineComponent({
    setup: () => () => [
      h("button", { id: "opener", onClick: () => { open.value = true; } }, "Add host"),
      h(Dialog, {
        open: open.value, title: "Add host", ...props,
        onClose: () => { closes.value += 1; open.value = false; },
      }, { default: () => h("input", { id: "label" }) }),
    ],
  });
  wrapper = mount(Host, { attachTo: document.body });
  return { open, closes };
}

const panel = () => document.querySelector<HTMLElement>('[role="dialog"]')!;

describe("Dialog keyboard and dismissal", () => {
  it("closes on Escape", async () => {
    const { closes } = mountForm();
    document.getElementById("opener")!.click();
    await flushPromises();
    panel().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(closes.value).toBe(1);
  });

  it("keeps a form open when the backdrop is clicked", async () => {
    const { closes } = mountForm();
    document.getElementById("opener")!.click();
    await flushPromises();
    (document.querySelector("[data-terminal-focus-blocker]") as HTMLElement).click();
    expect(closes.value).toBe(0);
    expect(panel()).not.toBeNull();
  });

  it("closes on a backdrop click when asked to", async () => {
    const { closes } = mountForm({ dismissOnOverlay: true });
    document.getElementById("opener")!.click();
    await flushPromises();
    (document.querySelector("[data-terminal-focus-blocker]") as HTMLElement).click();
    expect(closes.value).toBe(1);
  });

  it("brings Tab back into the dialog when focus has fallen to the page", async () => {
    mountForm();
    document.getElementById("opener")!.click();
    await flushPromises();
    (document.activeElement as HTMLElement | null)?.blur();
    const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    document.body.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true);
    expect(panel().contains(document.activeElement)).toBe(true);
  });

  it("returns focus to the control that opened it", async () => {
    mountForm();
    const opener = document.getElementById("opener")!;
    opener.focus();
    opener.click();
    await flushPromises();
    expect(document.activeElement).not.toBe(opener);
    panel().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await nextTick();
    expect(document.activeElement).toBe(opener);
  });
});
