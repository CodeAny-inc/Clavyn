import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import TerminalArea from "./TerminalArea.vue";
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
  vi.restoreAllMocks();
});

describe("empty terminal view", () => {
  it("offers to start a session from the middle of the view", async () => {
    const showModal = vi.fn();
    vi.spyOn(HTMLDialogElement.prototype, "showModal").mockImplementation(showModal);
    wrapper = mount(TerminalArea, { props: { visible: true }, attachTo: document.body });
    await flushPromises();

    const empty = wrapper.get(".terminal-empty");
    const start = empty.findAll("button").find((b) => b.text() === "New session");
    expect(start).toBeDefined();
    await start!.trigger("click");
    expect(showModal).toHaveBeenCalledTimes(1);
  });
});
