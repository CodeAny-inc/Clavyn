// Every destructive action waits for an answer before it touches anything. The
// webview's `confirm` returns a Promise, so a caller that tested its result
// directly went ahead whatever the user chose.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import HostList from "./HostList.vue";
import KeyManager from "./KeyManager.vue";
import IdentityManager from "./IdentityManager.vue";
import WorkspaceView from "./WorkspaceView.vue";
import SftpBrowser from "./SftpBrowser.vue";
import { answerConfirmation, pendingConfirmation } from "../composables/useConfirm";
import { useVaultStore } from "../stores/vault";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Host, Identity, KeyMeta, Workspace } from "../types";

const host: Host = {
  id: "atlas", label: "Atlas", hostname: "atlas.example.test", port: 22,
  username: "deploy", auth: { password: { credential_key: "fixture-only" } }, tags: [],
};
const key: KeyMeta = {
  id: "key-1", label: "laptop", key_type: "ed25519",
  fingerprint: "SHA256:fixture", public_key_base64: "AAAAC3NzaC1lZDI1NTE5AAAAIfixture",
};
const identity: Identity = {
  id: "ident-1", label: "Deploy", username: "deploy",
  auth: { password: { credential_key: "fixture-only" } }, tags: [],
};
const workspace: Workspace = { id: "ws-1", name: "Release", tabs: [] };
const readme = {
  name: "README.md", long_name: "README.md", is_dir: false, is_file: true,
  is_symlink: false, size: 4, modified: null, permissions: null,
};

const calls = (command: string) => getInvokeMock().mock.calls.filter(([name]) => name === command);
let wrapper: VueWrapper | undefined;

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  setInvokeHandler("list_groups", () => []);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("list_workspaces", () => []);
  setInvokeHandler("list_hosts", () => [host]);
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
});

/** Waits for the question, checks it, and answers it. */
async function answer(accept: boolean, message: string) {
  await vi.waitFor(() => expect(pendingConfirmation.value).not.toBeNull());
  expect(pendingConfirmation.value!.message).toBe(message);
  answerConfirmation(accept);
  await flushPromises();
}

/**
 * Clicks `trigger`, declines, clicks again and confirms, checking that
 * `command` runs only after the confirmation.
 */
async function declineThenConfirm(trigger: () => Promise<unknown>, command: string, message: string) {
  await trigger();
  await flushPromises();
  expect(calls(command), `${command} ran before the user answered`).toHaveLength(0);
  await answer(false, message);
  expect(calls(command), `${command} ran after the user declined`).toHaveLength(0);

  await trigger();
  await answer(true, message);
  expect(calls(command)).toHaveLength(1);
  return calls(command)[0][1];
}

describe("destructive actions wait for confirmation", () => {
  it("deleting a host", async () => {
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();
    const args = await declineThenConfirm(
      () => wrapper!.get('button[aria-label="Delete host"]').trigger("click"),
      "delete_host",
      'Delete host "Atlas"?',
    );
    expect(args).toEqual({ id: "atlas" });
  });

  it("deleting a host group", async () => {
    setInvokeHandler("list_groups", () => [{ id: "group-1", name: "Production" }]);
    wrapper = mount(HostList, { attachTo: document.body });
    await flushPromises();
    await declineThenConfirm(
      () => wrapper!.get('button[aria-label="Delete group"]').trigger("click"),
      "delete_group",
      'Delete group "Production"? Hosts in this group will be ungrouped.',
    );
  });

  it("deleting a key from the vault", async () => {
    setInvokeHandler("list_keys", () => [key]);
    const vault = useVaultStore();
    vault.initialized = true;
    vault.unlocked = true;
    wrapper = mount(KeyManager, { attachTo: document.body });
    await flushPromises();
    const args = await declineThenConfirm(
      () => wrapper!.get('button[aria-label="Delete key"]').trigger("click"),
      "delete_key",
      'Delete key "laptop"? It is removed from the vault for good, and servers that only accept this key will refuse it.',
    );
    expect(args).toEqual({ keyId: "key-1" });
  });

  it("deleting an identity", async () => {
    setInvokeHandler("list_identities", () => [identity]);
    wrapper = mount(IdentityManager, { attachTo: document.body });
    await flushPromises();
    const args = await declineThenConfirm(
      () => wrapper!.get('button[aria-label="Delete identity"]').trigger("click"),
      "delete_identity",
      'Delete identity "Deploy"?',
    );
    expect(args).toEqual({ id: "ident-1" });
  });

  it("deleting a group from the identities view", async () => {
    setInvokeHandler("list_groups", () => [{ id: "group-1", name: "Production" }]);
    wrapper = mount(IdentityManager, { attachTo: document.body });
    await flushPromises();
    await declineThenConfirm(
      () => wrapper!.get('button[aria-label="Delete group"]').trigger("click"),
      "delete_group",
      'Delete group "Production"? Identities in this group will be ungrouped.',
    );
  });

  it("deleting a workspace", async () => {
    setInvokeHandler("list_workspaces", () => [workspace]);
    wrapper = mount(WorkspaceView, { attachTo: document.body });
    await flushPromises();
    const args = await declineThenConfirm(
      () => wrapper!.get('button[aria-label="Delete workspace"]').trigger("click"),
      "delete_workspace",
      'Delete workspace "Release"?',
    );
    expect(args).toEqual({ id: "ws-1" });
  });

  describe("on a connected SFTP session", () => {
    beforeEach(async () => {
      setInvokeHandler("sftp_canonicalize", () => "/home/deploy");
      setInvokeHandler("sftp_list_dir", () => [readme]);
      wrapper = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
      await flushPromises();
      await wrapper.get("select").setValue(host.id);
      await wrapper.get('input[type="password"]').setValue("fixture-password");
      await wrapper.findAll("button").find((button) => button.text().includes("Connect"))!.trigger("click");
      await vi.waitFor(() => expect(wrapper!.text()).toContain("README.md"));
    });

    it("deleting a remote file", async () => {
      const args = await declineThenConfirm(async () => {
        const row = wrapper!.findAll("tr").find((item) => item.text().includes("README.md"))!;
        await row.trigger("contextmenu");
        await wrapper!.findAll("button").find((button) => button.text() === "Delete")!.trigger("click");
      }, "sftp_remove_file", 'Delete "README.md"?');
      expect(args).toMatchObject({ path: "/home/deploy/README.md" });
    });

    it("overwriting a remote file on upload", async () => {
      setInvokeHandler("sftp_pick_upload_file", () => ({ token: "upload-token", name: "README.md" }));
      const args = await declineThenConfirm(
        () => wrapper!.get('button[title="Upload"]').trigger("click"),
        "sftp_upload_from_local",
        '"README.md" already exists on the remote host. Overwrite it?',
      );
      expect(args).toMatchObject({ uploadToken: "upload-token", overwrite: true });
    });
  });
});

describe("failures are reported in the page", () => {
  it("shows a key file that could not be read inside the identity form", async () => {
    setInvokeHandler("pick_key_file", () => { throw new Error("permission denied"); });
    wrapper = mount(IdentityManager, { attachTo: document.body });
    await flushPromises();

    await wrapper.findAll("button").find((button) => button.text().includes("Add Identity"))!.trigger("click");
    const form = () => document.querySelector<HTMLElement>('[role="dialog"]')!;
    const select = form().querySelector("select")!;
    select.value = "publickey";
    select.dispatchEvent(new Event("change"));
    await flushPromises();
    [...form().querySelectorAll("button")].find((button) => button.textContent?.includes("Import"))!.click();
    await flushPromises();
    [...form().querySelectorAll("button")].find((button) => button.textContent?.includes("Browse"))!.click();
    await flushPromises();

    expect(form().querySelector('[role="alert"]')?.textContent).toContain("Failed to read key file: Error: permission denied");
  });
});
