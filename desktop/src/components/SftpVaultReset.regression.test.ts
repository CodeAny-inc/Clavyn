import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import SftpBrowser from "./SftpBrowser.vue";
import { useSftpStore } from "../stores/sftp";
import { useVaultStore } from "../stores/vault";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host } from "../types";

const host: Host = {
  id: "atlas", label: "Atlas", hostname: "atlas.example.test", port: 22, username: "deploy",
  auth: "agent", tags: [],
};
const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  setInvokeHandler("list_hosts", () => [host]);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("sftp_canonicalize", () => "/home/deploy");
  setInvokeHandler("sftp_list_dir", () => [
    { name: "notes.txt", long_name: "notes.txt", is_dir: false, is_file: true, is_symlink: false, size: 3, modified: null, permissions: null },
  ]);
  setInvokeHandler("biometric_passphrase_stored", () => false);
});
afterEach(() => {
  wrapper?.unmount(); wrapper = undefined;
});

describe("SFTP state after a vault reset", () => {
  it("clears the published SFTP session without asking the backend to close it", async () => {
    const sftp = useSftpStore();
    const vault = useVaultStore();
    vault.initialized = true;
    vault.unlocked = true;
    await sftp.connect(host);
    expect(sftp.sessionId).not.toBeNull();

    await vault.reset("passphrase");

    expect(sftp.sessionId).toBeNull();
    expect(sftp.entries).toEqual([]);
    expect(sftp.currentPath).toBe("/");
    expect(calls("sftp_close")).toHaveLength(0);
  });

  it("returns the Files view to the connect form", async () => {
    wrapper = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
    await flushPromises();
    await wrapper.get("select").setValue(host.id);
    await wrapper.findAll("button").find(button => button.text().includes("Connect"))!.trigger("click");
    await vi.waitFor(() => expect(wrapper!.text()).toContain("notes.txt"));

    await useVaultStore().reset("passphrase");
    await flushPromises();

    expect(wrapper.text()).not.toContain("notes.txt");
    expect(wrapper.find("select").exists()).toBe(true);
  });
});
