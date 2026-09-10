import { describe, expect, it } from "vitest";
import tauriConfig from "../../src-tauri/tauri.conf.json";

describe("Windows WebView2 install mode", () => {
  // With no explicit mode the bundler falls back to downloadBootstrapper, which
  // fetches MicrosoftEdgeWebview2Setup.exe through NSISdl and runs it. NSISdl
  // speaks HTTP/1.0 over raw sockets with no TLS, and nothing checks the file's
  // signature or hash before it executes.
  it("never resolves to the downloading bootstrapper", () => {
    const mode = (tauriConfig as Record<string, any>).bundle?.windows
      ?.webviewInstallMode?.type;
    expect(["embedBootstrapper", "offlineInstaller"]).toContain(mode);
  });
});
