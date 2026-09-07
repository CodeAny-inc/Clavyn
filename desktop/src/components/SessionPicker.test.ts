import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { useTabsStore, collectPanes } from "../stores/tabs";
import { useHostsStore } from "../stores/hosts";
import SessionPicker from "./SessionPicker.vue";
import AppSidebar from "./AppSidebar.vue";
import { getInvokeMock } from "../test/setup";

let wrapper: VueWrapper | undefined;
beforeEach(() => {
  setActivePinia(createPinia());
  useHostsStore().hosts = [
    { id: "atlas", label: "Atlas", hostname: "atlas.example.test", username: "demo", port: 22, auth: "agent", tags: ["production"] },
    { id: "orion", label: "Orion", hostname: "orion.example.test", username: "demo", port: 22, auth: "agent", tags: ["staging"] },
  ];
});
afterEach(() => { wrapper?.unmount(); wrapper = undefined; });
async function open(where: "tab" | "horizontal" | "vertical" = "tab", target?: string) {
  const picker = mount(SessionPicker, { attachTo: document.body });
  wrapper = picker;
  await picker.vm.show(where, target);
  await flushPromises();
  return picker;
}
describe("unified session creation", () => {
  it("opens a local shell from the same picker as saved hosts", async () => {
    const picker = await open();
    await picker.get('[aria-label="Open local shell"]').trigger("click");
    expect(useTabsStore().activePane?.terminalType).toBe("local");
    expect(picker.emitted("opened")).toHaveLength(1);
  });
  it.each(["horizontal", "vertical"] as const)("adds a host to the captured %s target, preserving its session", async direction => {
    const tabs = useTabsStore();
    const original = tabs.newTab(useHostsStore().hosts[0]);
    const target = tabs.activePaneId!;
    tabs.setPaneConnected(target, "original-session");
    const picker = await open(direction, target);
    tabs.newTab();
    await picker.get('[aria-label="Connect Orion"]').trigger("click");
    expect(collectPanes(tabs.tabs.find(tab => tab.id === original.id)!.tree).map(pane => pane.sessionId)).toEqual(["original-session", null]);
    expect(tabs.activePane?.hostId).toBe("orion");
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "close_session")).toEqual([]);
  });
  it("falls back to a new tab when the captured pane no longer exists", async () => {
    const tabs = useTabsStore(); tabs.newTab();
    const id = tabs.activePaneId!;
    const picker = await open("horizontal", id);
    tabs.closePane(id);
    await picker.get('[aria-label="Connect Orion"]').trigger("click");
    expect(tabs.tabs).toHaveLength(1);
    expect(tabs.activePane?.hostId).toBe("orion");
  });
  it("searches tags and connects with Enter only for one unambiguous result", async () => {
    const picker = await open();
    const input = picker.get('[aria-label="Search sessions"]');
    await input.trigger("keydown", { key: "Enter" });
    expect(useTabsStore().tabs).toHaveLength(0);
    await input.setValue("staging");
    expect(picker.findAll('[aria-label^="Connect "]')).toHaveLength(1);
    await input.trigger("keydown", { key: "Enter", isComposing: true });
    expect(useTabsStore().tabs).toHaveLength(0);
    await input.trigger("keydown", { key: "Enter" });
    expect(useTabsStore().activePane?.hostId).toBe("orion");
  });
  it("shows an honest empty search state without connecting", async () => {
    const picker = await open();
    await picker.get('[aria-label="Search sessions"]').setValue("missing-host");
    expect(picker.text()).toContain("No matching sessions");
    await picker.get('[aria-label="Search sessions"]').trigger("keydown", { key: "Enter" });
    expect(useTabsStore().tabs).toHaveLength(0);
  });
  it("keeps a single Vault entry and removes duplicate New Terminal navigation", () => {
    wrapper = mount(AppSidebar, { props: { activeView: "terminal", vaultUnlocked: true, tabCount: 2 } });
    expect(wrapper.findAll('[aria-label^="Vault,"]')).toHaveLength(1);
    expect(wrapper.text()).not.toContain("New Terminal");
    expect(wrapper.get('[aria-label="Terminal"]').attributes("aria-current")).toBe("page");
  });
});
