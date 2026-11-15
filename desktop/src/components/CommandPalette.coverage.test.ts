import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import CommandPalette from "./CommandPalette.vue";
import { useHostsStore } from "../stores/hosts";
import { useTabsStore } from "../stores/tabs";
import { useVaultStore } from "../stores/vault";
import { useSettingsStore } from "../stores/settings";
import type { Host } from "../types";

const host: Host = {
  id: "atlas", label: "Atlas Production", hostname: "atlas.example.test", port: 22, username: "deploy",
  auth: { password: { credential_key: "fixture-only" } }, tags: ["prod"],
  group_id: null, key_id: null, startup_command: null, proxy_command: null,
  jump_host_id: null, identity_id: null,
};

let wrapper: VueWrapper | undefined;

async function open() {
  wrapper = mount(CommandPalette, { props: { open: false }, attachTo: document.body });
  await wrapper.setProps({ open: true });
  await flushPromises();
  return document.querySelector<HTMLInputElement>('input[role="combobox"]')!;
}

async function search(input: HTMLInputElement, text: string) {
  input.value = text;
  input.dispatchEvent(new Event("input"));
  await flushPromises();
}

const options = () => [...document.querySelectorAll<HTMLElement>('[role="option"]')].map((o) => o.textContent ?? "");

beforeEach(() => {
  setActivePinia(createPinia());
  useHostsStore().hosts = [host];
});

afterEach(() => {
  vi.restoreAllMocks();
  wrapper?.unmount();
  wrapper = undefined;
  document.body.innerHTML = "";
});

describe("command palette coverage", () => {
  it("opens a saved host by name, address or tag", async () => {
    const input = await open();
    for (const text of ["atlas", "example.test", "prod"]) {
      await search(input, text);
      expect(options().some((o) => o.includes("Connect to Atlas Production")), text).toBe(true);
    }
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
    expect(useTabsStore().tabs.some((t) => t.title.includes("Atlas"))).toBe(true);
    expect(wrapper!.emitted("navigate")?.[0]).toEqual(["terminal"]);
  });

  it("does not reveal a masked address while matching it", async () => {
    useSettingsStore().maskAddresses = true;
    const input = await open();
    await search(input, "atlas");
    expect(options().join(" ")).not.toContain("atlas.example.test");
  });

  it("reaches Files, and locks an unlocked vault", async () => {
    useVaultStore().unlocked = true;
    const input = await open();
    await search(input, "files");
    expect(options()).toEqual([expect.stringContaining("Go to Files")]);
    await search(input, "lock");
    expect(options().some((o) => o.includes("Lock Vault"))).toBe(true);
  });

  it("exposes the results as a listbox the input points into", async () => {
    const input = await open();
    const list = document.getElementById(input.getAttribute("aria-controls")!)!;
    expect(list.getAttribute("role")).toBe("listbox");
    const active = document.getElementById(input.getAttribute("aria-activedescendant")!)!;
    expect(active.getAttribute("aria-selected")).toBe("true");
  });
});

describe("command palette highlight when the results change", () => {
  const borealis: Host = { ...host, id: "borealis", label: "Borealis Staging", hostname: "borealis.example.test", tags: [] };

  const optionWithText = (text: string) =>
    [...document.querySelectorAll<HTMLElement>('[role="option"]')].find((o) => o.textContent?.includes(text))!;
  const highlighted = () => document.querySelector<HTMLElement>('[role="option"][aria-selected="true"]')?.textContent ?? "";

  it("keeps the highlighted host when a result above it disappears", async () => {
    useHostsStore().hosts = [host, borealis];
    useVaultStore().unlocked = true;
    const input = await open();
    optionWithText("Connect to Atlas Production").dispatchEvent(new MouseEvent("mouseenter"));
    await flushPromises();
    expect(highlighted()).toContain("Atlas Production");

    // The vault auto-locking drops "Lock Vault", which sits above every host.
    useVaultStore().unlocked = false;
    await flushPromises();
    expect(highlighted()).toContain("Atlas Production");

    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
    expect(useTabsStore().tabs.map((t) => t.title)).toEqual(["Atlas Production"]);
  });

  it("never points the input at an option that is no longer listed", async () => {
    useHostsStore().hosts = [host, borealis];
    const input = await open();
    optionWithText("Connect to Borealis Staging").dispatchEvent(new MouseEvent("mouseenter"));
    await flushPromises();

    useHostsStore().hosts = [host];
    await flushPromises();
    const active = document.getElementById(input.getAttribute("aria-activedescendant")!);
    expect(active?.getAttribute("aria-selected")).toBe("true");
    expect(highlighted()).toContain("Go to Hosts");
  });

  it("leaves no active descendant when nothing matches", async () => {
    const input = await open();
    await search(input, "zzz-no-such-command");
    expect(input.hasAttribute("aria-activedescendant")).toBe(false);
  });
});

describe("command palette vault lock", () => {
  async function runLockVault() {
    useVaultStore().unlocked = true;
    const input = await open();
    await search(input, "lock vault");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
  }

  it("closes once the vault is locked", async () => {
    const lock = vi.spyOn(useVaultStore(), "lock").mockResolvedValue(undefined);
    await runLockVault();
    expect(lock).toHaveBeenCalledTimes(1);
    expect(wrapper!.emitted("close")).toHaveLength(1);
  });

  it("stays open and says so when locking fails, so it can be retried", async () => {
    const lock = vi.spyOn(useVaultStore(), "lock").mockRejectedValue(new Error("backend unavailable"));
    vi.spyOn(console, "error").mockImplementation(() => {});
    await runLockVault();

    expect(wrapper!.emitted("close")).toBeUndefined();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Could not lock the vault");
    expect(useVaultStore().unlocked).toBe(true);
    expect(options().some((o) => o.includes("Lock Vault"))).toBe(true);

    lock.mockResolvedValue(undefined);
    document.querySelector<HTMLInputElement>('input[role="combobox"]')!
      .dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    await flushPromises();
    expect(document.querySelector('[role="alert"]')).toBeNull();
    expect(wrapper!.emitted("close")).toHaveLength(1);
  });
});
