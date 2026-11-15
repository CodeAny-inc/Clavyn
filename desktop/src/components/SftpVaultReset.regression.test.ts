import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import SftpBrowser from "./SftpBrowser.vue";
import { useSftpStore } from "../stores/sftp";
import { useVaultStore } from "../stores/vault";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { SftpEntry } from "../api";
import type { Host } from "../types";

const host: Host = {
  id: "atlas", label: "Atlas", hostname: "atlas.example.test", port: 22, username: "deploy",
  auth: "agent", tags: [],
};
const file: SftpEntry = {
  name: "notes.txt", long_name: "notes.txt", is_dir: false, is_file: true, is_symlink: false, size: 3, modified: null, permissions: null,
};
const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  setInvokeHandler("list_hosts", () => [host]);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("sftp_canonicalize", () => "/home/deploy");
  setInvokeHandler("sftp_list_dir", () => [file]);
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

  it("keeps the connect form when a connection that was pending during the reset finishes", async () => {
    let lateSessionId: string | undefined;
    let release!: () => void;
    setInvokeHandler("sftp_connect", (args) => {
      lateSessionId = args.sessionId;
      return new Promise<void>((resolve) => { release = resolve; });
    });
    wrapper = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
    await flushPromises();
    await wrapper.get("select").setValue(host.id);
    await wrapper.findAll("button").find(button => button.text().includes("Connect"))!.trigger("click");
    await vi.waitFor(() => expect(calls("sftp_connect")).toHaveLength(1));

    await useVaultStore().reset("passphrase");
    release();
    await flushPromises();

    const sftp = useSftpStore();
    expect(sftp.sessionId).toBeNull();
    expect(wrapper.find("select").exists()).toBe(true);
    expect(wrapper.findAll("button").some(button => button.text() === "Disconnect")).toBe(false);
    // The reset could not close a session that did not exist yet, so the
    // connection closes the late one itself.
    expect(calls("sftp_close")).toEqual([["sftp_close", { sessionId: lateSessionId }]]);
  });

  it("shows no error for a download that fails after the reset", async () => {
    const sftp = useSftpStore();
    await sftp.connect(host);
    let fail!: (reason: unknown) => void;
    setInvokeHandler("sftp_download_to_local", () => new Promise((_, reject) => { fail = reject; }));
    const download = sftp.downloadFile({ sessionId: sftp.sessionId!, dir: "/home/deploy", entry: file });
    const outcome = expect(download).rejects.toBe("the SFTP session was closed");
    await vi.waitFor(() => expect(calls("sftp_download_to_local")).toHaveLength(1));

    await useVaultStore().reset("passphrase");
    fail("the SFTP session was closed");
    await outcome;

    expect(sftp.error).toBeNull();
    expect(sftp.loading).toBe(false);
  });

  it("shows no error for an upload that fails after the reset", async () => {
    const sftp = useSftpStore();
    await sftp.connect(host);
    let fail!: (reason: unknown) => void;
    setInvokeHandler("sftp_upload_from_local", () => new Promise((_, reject) => { fail = reject; }));
    const upload = sftp.uploadFile("notes.txt", "upload-token");
    const outcome = expect(upload).rejects.toBe("the SFTP session was closed");
    await vi.waitFor(() => expect(calls("sftp_upload_from_local")).toHaveLength(1));

    await useVaultStore().reset("passphrase");
    fail("the SFTP session was closed");
    await outcome;

    expect(sftp.error).toBeNull();
    expect(sftp.loading).toBe(false);
  });
});
