import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import { useSftpStore } from "./sftp";
import type { SftpEntry } from "../api";
import type { Host } from "../types";

const host: Host = {
  id: "atlas",
  label: "Atlas",
  hostname: "atlas.example.test",
  port: 22,
  username: "deploy",
  auth: { password: { credential_key: "metadata-only" } },
  tags: [],
};

const file: SftpEntry = {
  name: "report.txt", long_name: "report.txt", is_dir: false, is_file: true,
  is_symlink: false, size: 12, modified: null, permissions: null,
};

const calls = (command: string) =>
  getInvokeMock().mock.calls.filter(([name]) => name === command);

beforeEach(async () => {
  setActivePinia(createPinia());
  setInvokeHandler("sftp_connect", () => undefined);
  setInvokeHandler("sftp_canonicalize", () => "/home/deploy");
  setInvokeHandler("sftp_list_dir", () => [file]);
  await useSftpStore().connect(host, "SECRET", "deploy");
});

// The local side of a transfer is chosen in a native dialog the backend opens,
// so nothing the page sends can name a local path.
describe("SFTP transfers", () => {
  it("downloads by remote path alone", async () => {
    setInvokeHandler("sftp_download_to_local", () => true);
    await useSftpStore().downloadFile(file);

    const [[, args]] = calls("sftp_download_to_local");
    expect(args).toEqual({ sessionId: expect.any(String), remotePath: "/home/deploy/report.txt" });
  });

  it("uploads the picked file by its token", async () => {
    setInvokeHandler("sftp_upload_from_local", () => undefined);
    await useSftpStore().uploadFile("report.txt", "picked-token", true);

    const [[, args]] = calls("sftp_upload_from_local");
    expect(args).toEqual({
      sessionId: expect.any(String),
      uploadToken: "picked-token",
      remotePath: "/home/deploy/report.txt",
      overwrite: true,
    });
  });
});
