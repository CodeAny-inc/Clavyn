import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import * as api from "../api";
import KnownHostsView from "./KnownHostsView.vue";

vi.mock("../api", () => ({
  listKnownHosts: vi.fn(),
  listRemovedKnownHosts: vi.fn(),
  listHostKeyChanges: vi.fn(),
  removeKnownHost: vi.fn(),
  forgetKnownHost: vi.fn(),
  replaceKnownHost: vi.fn(),
}));

const change = {
  host: "prod.example.com:22",
  key_type: "ssh-ed25519",
  pinned_fingerprint: "SHA256:pinnedfingerprint",
  presented_fingerprint: "SHA256:presentedfingerprint",
};

const removed = {
  host: "::1:22",
  key_type: "ssh-ed25519",
  fingerprint: "SHA256:retainedfingerprint",
};

function buttonLabelled(wrapper: VueWrapper, label: string) {
  const button = wrapper
    .findAll("button")
    .find((item) => item.text().trim() === label);
  expect(button, `Missing the "${label}" action`).toBeDefined();
  return button!;
}

function trustButton(wrapper: VueWrapper) {
  return buttonLabelled(wrapper, "Trust new key");
}

describe("KnownHostsView host key changes", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(api.listKnownHosts).mockResolvedValue([]);
    vi.mocked(api.listRemovedKnownHosts).mockResolvedValue([]);
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

describe("KnownHostsView removed hosts", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(api.listKnownHosts).mockResolvedValue([]);
    vi.mocked(api.listRemovedKnownHosts).mockResolvedValue([removed]);
    vi.mocked(api.listHostKeyChanges).mockResolvedValue([]);
    vi.mocked(api.forgetKnownHost).mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("shows the key a removed host is still remembered by", async () => {
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    const row = wrapper.get('[data-testid="removed-known-host"]');
    expect(row.text()).toContain(removed.fingerprint);
  });

  it("erases the retained key only after the user confirms", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    await buttonLabelled(wrapper, "Forget permanently").trigger("click");
    await flushPromises();
    expect(api.forgetKnownHost).not.toHaveBeenCalled();

    confirm.mockReturnValue(true);
    await buttonLabelled(wrapper, "Forget permanently").trigger("click");
    await flushPromises();
    // The host is a bare IPv6 address, so everything before the last colon is
    // the host and only the trailing number is the port.
    expect(api.forgetKnownHost).toHaveBeenCalledWith("::1", 22);
  });
});
