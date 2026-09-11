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

const trusted = {
  host: "[2001:db8::1]:2222",
  key_type: "ssh-ed25519",
  fingerprint: "SHA256:trustedfingerprint",
};

// The tag the backend puts on the error it raises when the native dialog was
// answered with Cancel.
const DECLINED = "[host-key-confirmation-declined] Forget host key: cancelled";

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

  it("leaves the confirmation to the backend instead of asking in the page", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    await trustButton(wrapper).trigger("click");
    await flushPromises();

    // A page-level confirm is not a control — a scripted `invoke` skips it — so
    // the only question asked is the native one the command raises.
    expect(confirm).not.toHaveBeenCalled();
    expect(api.replaceKnownHost).toHaveBeenCalledWith(
      "prod.example.com",
      22,
      change.presented_fingerprint,
    );
  });

  it("reports a refused pin but stays quiet when the dialog was cancelled", async () => {
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    vi.mocked(api.replaceKnownHost).mockRejectedValueOnce(
      "[host-key-confirmation-declined] Host key changed: cancelled",
    );
    await trustButton(wrapper).trigger("click");
    await flushPromises();
    expect(wrapper.text()).not.toContain("cancelled");

    vi.mocked(api.replaceKnownHost).mockRejectedValueOnce("no unreviewed host key");
    await trustButton(wrapper).trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("no unreviewed host key");
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

  it("sends the host and port back in the shape the entry was listed under", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    await buttonLabelled(wrapper, "Forget permanently").trigger("click");
    await flushPromises();

    expect(confirm).not.toHaveBeenCalled();
    // A bare IPv6 address: everything before the last colon is the host, so
    // "host:port" rebuilds the entry this row was listed under.
    expect(api.forgetKnownHost).toHaveBeenCalledWith("::1", 22);
  });

  it("stays quiet when the native confirmation was cancelled", async () => {
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    vi.mocked(api.forgetKnownHost).mockRejectedValueOnce(DECLINED);
    await buttonLabelled(wrapper, "Forget permanently").trigger("click");
    await flushPromises();
    expect(wrapper.text()).not.toContain("cancelled");

    vi.mocked(api.forgetKnownHost).mockRejectedValueOnce("is still trusted");
    await buttonLabelled(wrapper, "Forget permanently").trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("is still trusted");
  });

  it("reports a dialog that could not be shown instead of looking like a cancel", async () => {
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    // Failing closed is right; failing silently is not. Without this the button
    // does nothing at all on a platform where the native dialog never appears.
    vi.mocked(api.forgetKnownHost).mockRejectedValueOnce(
      "Forget host key: the confirmation dialog could not be shown, so nothing was changed",
    );
    await buttonLabelled(wrapper, "Forget permanently").trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("could not be shown");
  });
});

describe("KnownHostsView trusted hosts", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(api.listKnownHosts).mockResolvedValue([trusted]);
    vi.mocked(api.listRemovedKnownHosts).mockResolvedValue([]);
    vi.mocked(api.listHostKeyChanges).mockResolvedValue([]);
    vi.mocked(api.removeKnownHost).mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("keeps the brackets of an IPv6 entry so the pair rebuilds it", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    await wrapper.get('button[aria-label="Remove known host"]').trigger("click");
    await flushPromises();

    expect(api.removeKnownHost).toHaveBeenCalledWith("[2001:db8::1]", 2222);
  });

  it("reports a removal that failed instead of dropping it", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    vi.mocked(api.removeKnownHost).mockRejectedValueOnce("known_hosts.json is read-only");
    await wrapper.get('button[aria-label="Remove known host"]').trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("known_hosts.json is read-only");
  });

  it("says a search matched nothing instead of rendering an empty list", async () => {
    vi.mocked(api.listRemovedKnownHosts).mockResolvedValue([removed]);
    const wrapper = mount(KnownHostsView);
    await flushPromises();

    await wrapper.get("input").setValue("nothing-matches-this");
    await flushPromises();

    expect(wrapper.text()).toContain("No trusted hosts match");
  });
});
