import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import WorkspaceView from "./WorkspaceView.vue";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Workspace } from "../types";

const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);

function workspace(id: string, name: string): Workspace {
  return { id, name, tabs: [], icon: null, description: null, color: null, host_ids: [], auto_connect: false };
}

describe("WorkspaceView", () => {
  let wrapper: VueWrapper;

  async function mountView(existing: Workspace[] = []) {
    setInvokeHandler("list_workspaces", () => existing);
    wrapper = mount(WorkspaceView, { global: { stubs: { Teleport: true } } });
    await flushPromises();
  }

  function button(text: string) {
    const found = wrapper.findAll("button").find((b) => b.text() === text);
    expect(found, `Missing the "${text}" button`).toBeDefined();
    return found!;
  }

  beforeEach(() => {
    setActivePinia(createPinia());
    setInvokeHandler("list_hosts", () => []);
    setInvokeHandler("list_groups", () => []);
  });

  afterEach(() => {
    if (wrapper?.exists()) wrapper.unmount();
  });

  it("creates one workspace when Create workspace is clicked twice", async () => {
    await mountView();
    let finish!: () => void;
    setInvokeHandler("create_workspace", ({ name }) => new Promise((resolve) => {
      finish = () => resolve(workspace("ws-1", name));
    }));
    setInvokeHandler("save_workspace", ({ workspace: saved }) => saved);
    await button("New Workspace").trigger("click");
    await wrapper.get("#ws-name").setValue("Prod");

    await button("Create workspace").trigger("click");
    await button("Create workspace").trigger("click");
    await flushPromises();
    finish();
    await flushPromises();

    expect(calls("create_workspace")).toHaveLength(1);
    expect(calls("save_workspace")).toHaveLength(1);
    expect(wrapper.find("#ws-name").exists()).toBe(false);
  });

  it("keeps the dialog open on a failed save and retries without creating another workspace", async () => {
    await mountView();
    setInvokeHandler("create_workspace", ({ name }) => workspace("ws-1", name));
    let attempts = 0;
    setInvokeHandler("save_workspace", ({ workspace: saved }) => {
      attempts += 1;
      if (attempts === 1) throw new Error("disk full");
      return saved;
    });
    await button("New Workspace").trigger("click");
    await wrapper.get("#ws-name").setValue("Prod");

    await button("Create workspace").trigger("click");
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain("Failed to save workspace: Error: disk full");
    expect(wrapper.find("#ws-name").exists()).toBe(true);

    await button("Save changes").trigger("click");
    await flushPromises();

    expect(calls("create_workspace")).toHaveLength(1);
    expect(calls("save_workspace")).toHaveLength(2);
    expect(calls("save_workspace")[1][1].workspace.id).toBe("ws-1");
    expect(wrapper.find("#ws-name").exists()).toBe(false);
  });

  it("shows the error when deleting a workspace fails", async () => {
    await mountView([workspace("ws-1", "Prod")]);
    setInvokeHandler("delete_workspace", () => {
      throw new Error("store locked");
    });

    await wrapper.get('[aria-label="Delete workspace"]').trigger("click");
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain("Failed to delete workspace: Error: store locked");
    expect(wrapper.text()).toContain("Prod");
  });

  it("shows the error when saving the current layout fails", async () => {
    await mountView([workspace("ws-1", "Prod")]);
    setInvokeHandler("save_workspace", () => {
      throw new Error("disk full");
    });

    await wrapper.get('[aria-label="Save current layout"]').trigger("click");
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain("Failed to save layout: Error: disk full");
  });

  it("keeps the host picker open when saving associations fails", async () => {
    await mountView([workspace("ws-1", "Prod")]);
    setInvokeHandler("save_workspace", () => {
      throw new Error("disk full");
    });

    await wrapper.get('[aria-label="Manage hosts"]').trigger("click");
    await button("Save associations").trigger("click");
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain("Failed to save associations: Error: disk full");
    expect(wrapper.findAll("button").some((b) => b.text() === "Save associations")).toBe(true);
  });
});
