import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import HostList from "./HostList.vue";
import IdentityManager from "./IdentityManager.vue";
import WorkspaceView from "./WorkspaceView.vue";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import { useHostsStore } from "../stores/hosts";
import { useWorkspacesStore } from "../stores/workspaces";
import type { Host } from "../types";

const host: Host = {
  id: "atlas", label: "Atlas", hostname: "atlas.example.test", port: 22, username: "deploy",
  auth: { password: { credential_key: "fixture-only" } }, tags: [],
  group_id: null, key_id: null, startup_command: null, proxy_command: null,
  jump_host_id: null, identity_id: null,
};

let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  setInvokeHandler("list_hosts", () => []);
  setInvokeHandler("list_groups", () => []);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("list_workspaces", () => []);
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
});

// An empty list means "nothing stored" only once a read has succeeded. Until
// then it is "loading", and after a failed read it is an error; showing the
// first-run state in either case tells the user their data is gone.
describe("host list states", () => {
  it("reports a failed read with a Retry instead of looking empty", async () => {
    let fail = true;
    setInvokeHandler("list_hosts", () => {
      if (fail) throw new Error("store.json: Input/output error");
      return [host];
    });
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();

    const alert = wrapper.get('[role="alert"]');
    expect(alert.text()).toContain("Hosts could not be loaded");
    expect(alert.text()).toContain("Input/output error");
    expect(wrapper.text()).not.toContain("No hosts yet");

    fail = false;
    await alert.get("button").trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("Atlas");
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });

  it("says it is loading while the first read is in flight", async () => {
    setInvokeHandler("list_hosts", () => new Promise(() => {}));
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();

    expect(wrapper.text()).toContain("Loading hosts…");
    expect(wrapper.text()).not.toContain("No hosts yet");
  });

  it("says a search matched nothing and offers to clear it", async () => {
    setInvokeHandler("list_hosts", () => [host]);
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();

    await wrapper.get('input[placeholder="Search hosts..."]').setValue("zzz");
    expect(wrapper.text()).toContain('No hosts match "zzz"');
    expect(wrapper.text()).not.toContain("No hosts yet");

    const showAll = wrapper.findAll("button").find((b) => b.text() === "Show all hosts");
    await showAll!.trigger("click");
    expect(wrapper.text()).toContain("Atlas");
  });

  it("reports a reload that failed while earlier rows are still shown", async () => {
    let fail = false;
    setInvokeHandler("list_hosts", () => {
      if (fail) throw new Error("store.json: Input/output error");
      return [host];
    });
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();

    fail = true;
    await useHostsStore().load().catch(() => {});
    await flushPromises();
    expect(wrapper.text()).toContain("Atlas");
    expect(wrapper.get('[role="alert"]').text()).toContain("The host list could not be refreshed");
  });

  it("shows one alert when a filter hides every row and a reload fails", async () => {
    setInvokeHandler("list_hosts", () => [host]);
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();
    await wrapper.get('input[placeholder="Search hosts..."]').setValue("zzz");

    setInvokeHandler("list_hosts", () => {
      throw new Error("store.json: Input/output error");
    });
    await useHostsStore().load().catch(() => {});
    await flushPromises();
    expect(wrapper.text()).toContain('No hosts match "zzz"');
    expect(wrapper.findAll('[role="alert"]')).toHaveLength(1);
  });

  it("names the failed read when only the group list could not be loaded", async () => {
    setInvokeHandler("list_hosts", () => [host]);
    setInvokeHandler("list_groups", () => {
      throw new Error("store.json: Permission denied");
    });
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();

    const alert = wrapper.get('[role="alert"]');
    expect(alert.text()).toContain("groups:");
    expect(alert.text()).toContain("Permission denied");
    expect(alert.text()).not.toContain("hosts:");
  });

  it("keeps the first-run state for an account that really has no hosts", async () => {
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();
    expect(wrapper.text()).toContain("No hosts yet");
  });
});

describe("identity and workspace list states", () => {
  it("reports identities that could not be read", async () => {
    setInvokeHandler("list_identities", () => {
      throw new Error("denied");
    });
    wrapper = mount(IdentityManager, { attachTo: document.body });
    await flushPromises();

    expect(wrapper.text()).toContain("Identities could not be loaded");
    expect(wrapper.text()).not.toContain("No identities yet");
  });

  it("reports workspaces that could not be read", async () => {
    setInvokeHandler("list_workspaces", () => {
      throw new Error("store.json: Permission denied");
    });
    wrapper = mount(WorkspaceView, { attachTo: document.body });
    await flushPromises();

    expect(wrapper.text()).toContain("Workspaces could not be loaded");
    expect(wrapper.text()).toContain("Permission denied");
    expect(wrapper.text()).not.toContain("No workspaces yet");
  });

  it("lists every workspace, including one created while the startup read was in flight", async () => {
    const saved = { id: "ws-old", name: "release", tabs: [] };
    const created = { id: "ws-new", name: "ops", tabs: [] };
    const reads: Array<(items: unknown[]) => void> = [];
    setInvokeHandler("list_workspaces", () => new Promise((resolve) => reads.push(resolve)));
    setInvokeHandler("create_workspace", () => created);
    const store = useWorkspacesStore();
    const load = store.load();
    await store.createWorkspace("ops");
    // The first read was taken before the create and is repeated; the backend
    // answers the repeat with both.
    reads[0]([saved]);
    await vi.waitFor(() => expect(reads).toHaveLength(2));
    reads[1]([saved, created]);
    await load;
    expect(store.workspaces.map((w) => w.name)).toEqual(["release", "ops"]);
  });

  it("shares one workspace read between overlapping loads", async () => {
    let release!: () => void;
    setInvokeHandler("list_workspaces", () => new Promise((resolve) => { release = () => resolve([]); }));
    const store = useWorkspacesStore();
    const first = store.load();
    const second = store.load();
    release();
    await Promise.all([first, second]);
    const reads = getInvokeMock().mock.calls.filter((call) => call[0] === "list_workspaces");
    expect(reads).toHaveLength(1);
    expect(store.loadError).toBe("");
    expect(store.loaded).toBe(true);
  });
});
