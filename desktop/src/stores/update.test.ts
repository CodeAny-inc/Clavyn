import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { listen } from "@tauri-apps/api/event";
import { setInvokeHandler, getInvokeMock } from "../test/setup";
import { useUpdateStore } from "./update";

const AVAILABLE = {
  available: true,
  version: "0.1.2-alpha.5",
  current_version: "0.1.2-alpha.4",
  date: "2026-01-01",
  body: "Fixes",
};

const UP_TO_DATE = {
  available: false,
  version: "0.1.2-alpha.4",
  current_version: "0.1.2-alpha.4",
  date: null,
  body: null,
};

describe("update store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    localStorage.removeItem("clavyn.dismissedVersions");
    vi.mocked(listen).mockClear();
  });

  it("raises the notification from the check command", async () => {
    setInvokeHandler("check_for_updates", () => AVAILABLE);
    const store = useUpdateStore();

    await store.check();

    expect(store.available).toBe(true);
    expect(store.version).toBe("0.1.2-alpha.5");
    expect(store.currentVersion).toBe("0.1.2-alpha.4");
    expect(store.shouldNotify).toBe(true);
    expect(store.lastChecked).not.toBeNull();
  });

  it("stays quiet when the backend reports no update", async () => {
    setInvokeHandler("check_for_updates", () => UP_TO_DATE);
    const store = useUpdateStore();

    await store.check();

    expect(store.available).toBe(false);
    expect(store.shouldNotify).toBe(false);
  });

  it("records a failed check without claiming an update", async () => {
    setInvokeHandler("check_for_updates", () => {
      throw new Error("github api returned 503");
    });
    const store = useUpdateStore();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});

    await store.check();

    expect(store.error).toContain("github api returned 503");
    expect(store.available).toBe(false);
    expect(store.shouldNotify).toBe(false);
    expect(store.checking).toBe(false);
    consoleError.mockRestore();
  });

  it("subscribes only to the install lifecycle, not to an availability event", async () => {
    const store = useUpdateStore();

    await store.registerListeners();

    const events = vi.mocked(listen).mock.calls.map(call => call[0]);
    expect(events).toEqual(["update-progress", "update-extracting"]);
  });

  it("checks once while a check is already in flight", async () => {
    let release: (() => void) | null = null;
    setInvokeHandler(
      "check_for_updates",
      () => new Promise(resolve => { release = () => resolve(AVAILABLE); }),
    );
    const store = useUpdateStore();

    const first = store.check();
    const second = store.check();
    release!();
    await Promise.all([first, second]);

    const checks = getInvokeMock().mock.calls.filter(call => call[0] === "check_for_updates");
    expect(checks).toHaveLength(1);
    expect(store.shouldNotify).toBe(true);
  });
});
