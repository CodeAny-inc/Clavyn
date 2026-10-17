import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import SettingsView from "./SettingsView.vue";
import HostForm from "./HostForm.vue";
import { setInvokeHandler } from "../test/setup";

let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  setInvokeHandler("list_hosts", () => []);
  setInvokeHandler("list_groups", () => []);
  setInvokeHandler("list_identities", () => []);
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  document.body.innerHTML = "";
});

// A control a screen reader announces only as "switch" or "combobox" can't be
// told apart from its neighbours.
function unnamed(root: ParentNode, selector: string) {
  return [...root.querySelectorAll<HTMLElement>(selector)].filter((el) => {
    if (el.getAttribute("aria-label") || el.getAttribute("aria-labelledby")) return false;
    return !(el.id && root.querySelector(`label[for="${el.id}"]`));
  });
}

describe("accessible names", () => {
  it("names every switch and select in Settings", async () => {
    wrapper = mount(SettingsView, { attachTo: document.body });
    await flushPromises();
    expect(unnamed(document, '[role="switch"], select')).toEqual([]);
  });

  it("gives every button in Settings an accessible name", async () => {
    wrapper = mount(SettingsView, { attachTo: document.body });
    await flushPromises();
    const nameless = [...document.querySelectorAll<HTMLElement>("button")].filter((el) => {
      if (el.getAttribute("aria-label")?.trim()) return false;
      const visible = el.cloneNode(true) as HTMLElement;
      visible.querySelectorAll(".hidden").forEach((hidden) => hidden.remove());
      return !visible.textContent?.trim();
    });
    expect(nameless.map((el) => el.outerHTML)).toEqual([]);
  });

  it("links the host form's authentication method select to its label", async () => {
    wrapper = mount(HostForm, { props: { host: null }, attachTo: document.body });
    await flushPromises();
    expect(unnamed(document, "select")).toEqual([]);
  });
});
