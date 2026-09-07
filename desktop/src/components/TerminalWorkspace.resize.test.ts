import { expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import TerminalWorkspace from "./TerminalWorkspace.vue";
import { useTabsStore } from "../stores/tabs";
import { useUiStore } from "../stores/ui";
vi.mock("./TerminalPane.vue", () => ({ default: { props: ["pane", "tabId", "visible"], template: '<div class="xterm"><textarea /></div>' } }));

it("continues a pointer drag after the first reactive resize and stops on pointerup", async () => {
  setActivePinia(createPinia());
  const tabs = useTabsStore();
  tabs.newTab();
  tabs.splitPane(tabs.activePaneId!, "horizontal");
  const wrapper = mount(TerminalWorkspace, { props: { visible: true } });
  try {
    vi.spyOn(wrapper.element, "getBoundingClientRect").mockReturnValue({ x: 0, y: 0, left: 0, top: 0, right: 1000, bottom: 800, width: 1000, height: 800, toJSON() {} });
    const divider = wrapper.get('[role="separator"]');
    await divider.trigger("pointerdown", { button: 0, clientX: 500, clientY: 300 });
    window.dispatchEvent(new PointerEvent("pointermove", { clientX: 600 }));
    await nextTick();
    expect(divider.attributes("aria-valuenow")).toBe("60");
    window.dispatchEvent(new PointerEvent("pointermove", { clientX: 700 }));
    await nextTick();
    expect(divider.attributes("aria-valuenow")).toBe("70");
    window.dispatchEvent(new PointerEvent("pointerup"));
    window.dispatchEvent(new PointerEvent("pointermove", { clientX: 800 }));
    await nextTick();
    expect(divider.attributes("aria-valuenow")).toBe("70");
  } finally { wrapper.unmount(); vi.restoreAllMocks(); }
});

it("exits fullscreen before xterm can consume Escape or forward it to the session", async () => {
  setActivePinia(createPinia());
  const tabs = useTabsStore();
  const ui = useUiStore();
  tabs.newTab();
  const wrapper = mount(TerminalWorkspace, { props: { visible: true }, attachTo: document.body });
  try {
    ui.toggleFullscreen(tabs.activePaneId!);
    await nextTick();
    const input = wrapper.get("textarea").element;
    const terminalHandler = vi.fn();
    input.addEventListener("keydown", terminalHandler);
    const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    input.dispatchEvent(event);
    expect(ui.fullscreenPaneId).toBeNull();
    expect(terminalHandler).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
  } finally { wrapper.unmount(); }
});
