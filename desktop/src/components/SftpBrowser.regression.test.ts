import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import SftpBrowser from "./SftpBrowser.vue";
import { useSftpStore } from "../stores/sftp";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { SftpEntry } from "../api";
import type { Host } from "../types";

const host: Host = {
  id: "atlas", label: "Atlas", hostname: "atlas.example.test", port: 22,
  username: "u", auth: "agent", tags: [],
};

const entry = (name: string, is_dir = false): SftpEntry => ({
  name, long_name: name, is_dir, is_file: !is_dir, is_symlink: false,
  size: 1, modified: null, permissions: null,
});

const calls = (command: string) =>
  getInvokeMock().mock.calls.filter(([name]) => name === command);

let view: VueWrapper | undefined;
let releaseDocs: ((entries: SftpEntry[]) => void) | null;

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  releaseDocs = null;
  setInvokeHandler("list_hosts", () => [host]);
  setInvokeHandler("list_groups", () => []);
  setInvokeHandler("list_identities", () => []);
  setInvokeHandler("sftp_connect", () => undefined);
  setInvokeHandler("sftp_canonicalize", () => "/home/u");
  setInvokeHandler("sftp_list_dir", ({ path }: { path: string }) => {
    if (path === "/home/u/docs") {
      return new Promise<SftpEntry[]>(resolve => { releaseDocs = resolve; });
    }
    return [entry("docs", true), entry("notes.txt")];
  });
  setInvokeHandler("sftp_remove_file", () => undefined);
});

afterEach(() => {
  view?.unmount();
  view = undefined;
  vi.restoreAllMocks();
});

async function mountConnected() {
  view = mount(SftpBrowser, { props: { visible: true }, attachTo: document.body });
  await flushPromises();
  await view.get("select").setValue(host.id);
  const connect = view.findAll("button").find(button => button.text() === "Connect");
  await connect!.trigger("click");
  await flushPromises();
  expect(useSftpStore().currentPath).toBe("/home/u");
  return view;
}

const row = (wrapper: VueWrapper, name: string) =>
  wrapper.findAll("tbody tr").find(tr => tr.text().includes(name))!;

const menuButton = (wrapper: VueWrapper, label: string) =>
  wrapper.findAll(".fixed button").find(button => button.text() === label);

describe("SFTP browser entry actions across navigation", () => {
  it("closes the context menu once the directory it was opened on is replaced", async () => {
    const wrapper = await mountConnected();

    await row(wrapper, "docs").trigger("dblclick");
    await vi.waitFor(() => expect(releaseDocs).not.toBeNull());
    await row(wrapper, "notes.txt").trigger("contextmenu");
    expect(menuButton(wrapper, "Delete")).toBeDefined();

    releaseDocs!([entry("notes.txt")]);
    await flushPromises();

    expect(useSftpStore().currentPath).toBe("/home/u/docs");
    expect(menuButton(wrapper, "Delete")).toBeUndefined();
    expect(calls("sftp_remove_file")).toHaveLength(0);
  });

  it("closes the rename dialog once the directory changes underneath it", async () => {
    const wrapper = await mountConnected();

    await row(wrapper, "docs").trigger("dblclick");
    await vi.waitFor(() => expect(releaseDocs).not.toBeNull());
    await row(wrapper, "notes.txt").trigger("contextmenu");
    await menuButton(wrapper, "Rename")!.trigger("click");
    expect(wrapper.find('input[placeholder="New name"]').exists()).toBe(true);

    releaseDocs!([entry("notes.txt")]);
    await flushPromises();

    expect(wrapper.find('input[placeholder="New name"]').exists()).toBe(false);
  });

  it("deletes in the directory the menu was opened on and names the full path", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    const wrapper = await mountConnected();

    await row(wrapper, "notes.txt").trigger("contextmenu");
    await menuButton(wrapper, "Delete")!.trigger("click");
    await flushPromises();

    expect(confirm).toHaveBeenCalledWith('Delete "/home/u/notes.txt"?');
    expect(calls("sftp_remove_file").map(([, args]) => args.path)).toEqual(["/home/u/notes.txt"]);
  });
});
