import { describe, expect, it } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import StartupRecovery from "./StartupRecovery.vue";
import { getInvokeMock, setInvokeHandler } from "../test/setup";

const calls = (command: string) =>
  getInvokeMock().mock.calls.filter(([name]) => name === command);

const unreadableStore = {
  file: "/data/com.clavyn.app/store.json",
  file_name: "store.json",
  reason: "expected value at line 1 column 1",
};

describe("StartupRecovery", () => {
  it("names the unreadable file and why it could not be read", () => {
    const view = mount(StartupRecovery, { props: { failure: unreadableStore } });
    expect(view.text()).toContain("Clavyn could not load store.json");
    expect(view.get('[data-testid="startup-reason"]').text()).toBe(unreadableStore.reason);
    expect(view.text()).toContain(unreadableStore.file);
    expect(view.find('[data-testid="vault-note"]').exists()).toBe(false);
  });

  it("warns that a vault set aside takes the stored keys with it", () => {
    const view = mount(StartupRecovery, {
      props: { failure: { ...unreadableStore, file: "/data/vault.json", file_name: "vault.json" } },
    });
    expect(view.get('[data-testid="vault-note"]').text()).toContain("stored SSH keys");
  });

  it("moves the file aside, shows where it went, then restarts", async () => {
    setInvokeHandler("set_aside_unreadable_file", () => "/data/store.json.unreadable-1700000000");
    const view = mount(StartupRecovery, { props: { failure: unreadableStore } });

    await view.get("button").trigger("click");
    await flushPromises();
    expect(calls("set_aside_unreadable_file")).toHaveLength(1);
    // The page never names the file; the backend moves the one that failed.
    expect(calls("set_aside_unreadable_file")[0][1]).toBeUndefined();
    expect(view.get('[data-testid="moved-to"]').text()).toContain("store.json.unreadable-1700000000");

    const buttons = view.findAll("button");
    expect(buttons).toHaveLength(1);
    await buttons[0].trigger("click");
    expect(calls("restart_app")).toHaveLength(1);
  });

  it("offers only a retry when no single file is to blame", () => {
    const view = mount(StartupRecovery, {
      props: { failure: { file: null, file_name: null, reason: "permission denied" } },
    });
    const buttons = view.findAll("button");
    expect(buttons.map(b => b.text())).toEqual(["Try again"]);
    expect(view.text()).toContain("Clavyn could not load its saved data");
  });
});
