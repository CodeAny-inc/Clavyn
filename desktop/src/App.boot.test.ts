import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { flushPromises, shallowMount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import App from "./App.vue";
import { getInvokeMock, setInvokeHandler } from "./test/setup";
import { useHostsStore } from "./stores/hosts";
import { useIdentitiesStore } from "./stores/identities";
import { useKeysStore } from "./stores/keys";
import { useUpdateStore } from "./stores/update";
import { useVaultStore } from "./stores/vault";
import { useWorkspacesStore } from "./stores/workspaces";

const BOOT_READS: Record<string, unknown> = {
  vault_is_initialized: true,
  is_vault_unlocked: true,
  list_hosts: [],
  list_groups: [],
  list_keys: [],
  list_identities: [],
  list_workspaces: [],
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(resolvePromise => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

/** Blocks a command until the test releases it, so dispatch order is observable. */
function gate<T>(command: string, value: T) {
  const pending = deferred<T>();
  setInvokeHandler(command, () => pending.promise);
  return () => pending.resolve(value);
}

function commandsInvoked() {
  return getInvokeMock().mock.calls.map(call => call[0] as string);
}

describe("App boot", () => {
  let wrapper: VueWrapper;

  beforeEach(() => {
    setActivePinia(createPinia());
    setInvokeHandler("vault_is_initialized", () => true);
    setInvokeHandler("is_vault_unlocked", () => true);
    setInvokeHandler("biometric_available", () => false);
    setInvokeHandler("biometric_passphrase_stored", () => false);
    setInvokeHandler("list_hosts", () => [
      {
        id: "host-1",
        label: "Atlas",
        hostname: "atlas.example.test",
        port: 22,
        username: "deploy",
        group_id: null,
        key_id: null,
        auth: "agent",
        tags: [],
        startup_command: null,
        proxy_command: null,
        jump_host_id: null,
      },
    ]);
    setInvokeHandler("list_groups", () => [{ id: "group-1", name: "Production", color: null }]);
    setInvokeHandler("list_keys", () => [{ id: "key-1", label: "laptop", algo: "ed25519", fingerprint: "SHA256:x", public_key: "ssh-ed25519 AAAA", created_at: 0 }]);
    setInvokeHandler("list_identities", () => [
      { id: "identity-1", label: "deploy", username: "deploy", key_id: "key-1", group_id: null, tags: [] },
    ]);
    setInvokeHandler("list_workspaces", () => [{ id: "workspace-1", name: "default", layout: null }]);
    setInvokeHandler("check_for_updates", () => ({
      available: false,
      version: "0.0.0",
      current_version: "0.0.0",
      date: null,
      body: null,
    }));
  });

  afterEach(() => {
    wrapper?.unmount();
  });

  function mountApp() {
    wrapper = shallowMount(App);
    return wrapper;
  }

  it("dispatches every independent boot read before any of them resolves", async () => {
    const releases = Object.entries(BOOT_READS).map(([command, value]) => gate(command, value));
    mountApp();
    await flushPromises();

    const dispatched = commandsInvoked();
    for (const command of Object.keys(BOOT_READS)) {
      expect(dispatched, `${command} was not dispatched in the first round-trip`).toContain(command);
    }
    // The biometric probe reads the vault status that is still in flight, so it
    // must not have started yet.
    expect(dispatched).not.toContain("biometric_available");

    releases.forEach(release => release());
    await flushPromises();
  });

  it("populates every store the boot path loads", async () => {
    mountApp();
    await flushPromises();

    const vault = useVaultStore();
    expect(vault.initialized).toBe(true);
    expect(vault.unlocked).toBe(true);
    expect(useHostsStore().hosts).toHaveLength(1);
    expect(useHostsStore().groups).toHaveLength(1);
    expect(useKeysStore().keys).toHaveLength(1);
    expect(useIdentitiesStore().identities).toHaveLength(1);
    expect(useIdentitiesStore().loaded).toBe(true);
    expect(useWorkspacesStore().workspaces).toHaveLength(1);
  });

  it("probes biometric enrollment only after the vault status snapshot lands", async () => {
    const releaseInitialized = gate("vault_is_initialized", true);
    mountApp();
    await flushPromises();
    expect(commandsInvoked()).not.toContain("biometric_available");

    releaseInitialized();
    await flushPromises();

    const dispatched = commandsInvoked();
    expect(dispatched).toContain("biometric_available");
    expect(dispatched).toContain("biometric_passphrase_stored");
    expect(dispatched.indexOf("biometric_available")).toBeGreaterThan(
      dispatched.indexOf("is_vault_unlocked"),
    );
  });

  it("keeps the update check off the awaited boot path", async () => {
    const releaseUpdateCheck = gate("check_for_updates", {
      available: true,
      version: "9.9.9",
      current_version: "0.0.0",
      date: null,
      body: null,
    });
    mountApp();
    await flushPromises();

    const update = useUpdateStore();
    // Boot state is complete while the network request is still outstanding.
    expect(update.checking).toBe(true);
    expect(update.showModal).toBe(false);
    expect(useHostsStore().hosts).toHaveLength(1);
    expect(useWorkspacesStore().workspaces).toHaveLength(1);
    expect(useVaultStore().initialized).toBe(true);

    releaseUpdateCheck();
    await flushPromises();
    expect(update.available).toBe(true);
    expect(update.showModal).toBe(true);
  });
});
