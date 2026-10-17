import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import HostList from "./HostList.vue";
import { setInvokeHandler } from "../test/setup";
import type { Host } from "../types";

const HOST: Host = {
  id: "host-1", label: "Web", hostname: "web.example.com", port: 22, username: "deploy",
  group_id: "group-1", key_id: null, auth: "publickey", tags: [], identity_id: null,
};

describe("HostList failures", () => {
  let wrapper: VueWrapper;

  beforeEach(async () => {
    setActivePinia(createPinia());
    setInvokeHandler("list_hosts", () => [{ ...HOST }]);
    setInvokeHandler("list_groups", () => [{ id: "group-1", name: "Prod", color: null }]);
    wrapper = mount(HostList, { global: { stubs: { Teleport: true } } });
    await flushPromises();
  });

  afterEach(() => {
    if (wrapper.exists()) wrapper.unmount();
  });

  it("shows the error when deleting a host fails", async () => {
    setInvokeHandler("delete_host", () => {
      throw new Error("store locked");
    });

    await wrapper.get('[aria-label="Delete host"]').trigger("click");
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain("Failed to delete host: Error: store locked");
    expect(wrapper.text()).toContain("Web");
  });

  it("shows the error when deleting a group fails", async () => {
    setInvokeHandler("delete_group", () => {
      throw new Error("store locked");
    });

    await wrapper.get('[aria-label="Delete group"]').trigger("click");
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain("Failed to delete group: Error: store locked");
  });

  it("keeps the group dialog open when creating a group fails", async () => {
    setInvokeHandler("add_group", () => {
      throw new Error("disk full");
    });

    await wrapper.findAll("button").find((b) => b.text() === "Add group...")!.trigger("click");
    await wrapper.get("#group-name").setValue("Staging");
    await wrapper.findAll("button").find((b) => b.text() === "Create group")!.trigger("click");
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain("Failed to create group: Error: disk full");
    expect((wrapper.get("#group-name").element as HTMLInputElement).value).toBe("Staging");
  });
});
