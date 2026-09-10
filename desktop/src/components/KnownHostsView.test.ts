import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import * as api from "../api";
import KnownHostsView from "./KnownHostsView.vue";

vi.mock("../api", () => ({
  listKnownHosts: vi.fn(),
  listHostKeyChanges: vi.fn(),
  removeKnownHost: vi.fn(),
  replaceKnownHost: vi.fn(),
}));

const change = {
  host: "prod.example.com:22",
  key_type: "ssh-ed25519",
  pinned_fingerprint: "SHA256:pinnedfingerprint",
  presented_fingerprint: "SHA256:presentedfingerprint",
};

function trustButton(wrapper: VueWrapper) {
  const button = wrapper
    .findAll("button")
    .find((item) => item.text().trim() === "Trust new key");
  expect(button, "Missing the trust action for a changed host key").toBeDefined();
  return button!;
}

describe("KnownHostsView host key changes", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(api.listKnownHosts).mockResolvedValue([]);
    vi.mocked(api.listHostKeyChanges).mockResolvedValue([change]);
    vi.mocked(api.replaceKnownHost).mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("shows both fingerprints for a host whose key changed", async () => {
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    const row = wrapper.get('[data-testid="host-key-change"]');
    expect(row.text()).toContain(change.pinned_fingerprint);
    expect(row.text()).toContain(change.presented_fingerprint);
  });

  it("pins the reviewed fingerprint only after the user confirms", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    await trustButton(wrapper).trigger("click");
    await flushPromises();
    expect(api.replaceKnownHost).not.toHaveBeenCalled();

    confirm.mockReturnValue(true);
    await trustButton(wrapper).trigger("click");
    await flushPromises();
    expect(api.replaceKnownHost).toHaveBeenCalledWith(
      "prod.example.com",
      22,
      change.presented_fingerprint,
    );
  });
});
