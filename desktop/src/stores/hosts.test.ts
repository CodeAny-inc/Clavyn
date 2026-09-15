import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { useHostsStore } from "./hosts";
import type { Host, HostGroup } from "../types";

const mockHost: Host = {
  id: "host-1",
  label: "My Server",
  hostname: "example.com",
  port: 22,
  username: "root",
  group_id: null,
  key_id: null,
  auth: "publickey",
  tags: ["prod", "web"],
  startup_command: null,
  proxy_command: null,
  jump_host_id: null,
};

const mockHost2: Host = {
  id: "host-2",
  label: "Database",
  hostname: "db.internal",
  port: 2222,
  username: "admin",
  group_id: "group-1",
  key_id: null,
  auth: "publickey",
  tags: ["db"],
  startup_command: null,
  proxy_command: null,
  jump_host_id: null,
};

const mockGroup: HostGroup = {
  id: "group-1",
  name: "Production",
  color: null,
};

// Mock the api module — return fresh copies to prevent cross-test mutation
vi.mock("../api", () => ({
  listHosts: vi.fn(() =>
    Promise.resolve([
      { ...mockHost },
      { ...mockHost2 },
    ]),
  ),
  listGroups: vi.fn(() => Promise.resolve([{ ...mockGroup }])),
  addHost: vi.fn((host: Host) => Promise.resolve({ ...host, id: "new-host" })),
  updateHost: vi.fn((host: Host) => Promise.resolve({ ...host })),
  deleteHost: vi.fn(() => Promise.resolve()),
  addGroup: vi.fn((name: string) =>
    Promise.resolve({ id: "new-group", name, color: null }),
  ),
  deleteGroup: vi.fn(() => Promise.resolve()),
}));

import * as api from "../api";

describe("hosts store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  describe("load", () => {
    it("loads hosts and groups from the API", async () => {
      const store = useHostsStore();
      await store.load();

      expect(store.hosts).toHaveLength(2);
      expect(store.hosts[0].label).toBe("My Server");
      expect(store.groups).toHaveLength(1);
      expect(store.groups[0].name).toBe("Production");
    });

    it("does not let an in-flight read overwrite a change that finished first", async () => {
      // The snapshot this load returns was taken before the host below was
      // added, so writing it back would make the new row appear and then vanish.
      let releaseHosts: (hosts: Host[]) => void = () => {};
      vi.mocked(api.listHosts).mockReturnValueOnce(
        new Promise<Host[]>((resolve) => {
          releaseHosts = resolve;
        }),
      );

      const store = useHostsStore();
      const inFlight = store.load();

      await store.addHost({ ...mockHost, id: "host-3", label: "Added" });
      expect(store.hosts.map((h) => h.label)).toContain("Added");

      releaseHosts([{ ...mockHost }, { ...mockHost2 }]);
      await inFlight;

      expect(store.hosts.map((h) => h.label)).toContain("Added");
    });

    it("keeps the list a concurrent change did not touch", async () => {
      // The two reads are independent, so a host being added says nothing
      // about the group snapshot that came back with it. Discarding both would
      // leave groups empty until something else called load() again.
      let releaseHosts: (hosts: Host[]) => void = () => {};
      vi.mocked(api.listHosts).mockReturnValueOnce(
        new Promise<Host[]>((resolve) => {
          releaseHosts = resolve;
        }),
      );

      const store = useHostsStore();
      const inFlight = store.load();

      await store.addHost({ ...mockHost, id: "host-3", label: "Added" });

      releaseHosts([{ ...mockHost }, { ...mockHost2 }]);
      await inFlight;

      expect(store.hosts.map((h) => h.label)).toContain("Added");
      expect(store.groups).toHaveLength(1);
    });

    it("discards a group snapshot that a group change has overtaken", async () => {
      let releaseGroups: (groups: HostGroup[]) => void = () => {};
      vi.mocked(api.listGroups).mockReturnValueOnce(
        new Promise<HostGroup[]>((resolve) => {
          releaseGroups = resolve;
        }),
      );

      const store = useHostsStore();
      const inFlight = store.load();

      await store.addGroup("Staging");
      expect(store.groups.map((g) => g.name)).toContain("Staging");

      releaseGroups([{ ...mockGroup }]);
      await inFlight;

      expect(store.groups.map((g) => g.name)).toContain("Staging");
      expect(store.hosts).toHaveLength(2);
    });

    it("reads hosts and groups in one round-trip", async () => {
      const store = useHostsStore();
      const order: string[] = [];
      vi.mocked(api.listHosts).mockImplementationOnce(() => {
        order.push("hosts");
        return Promise.resolve([]);
      });
      vi.mocked(api.listGroups).mockImplementationOnce(() => {
        order.push("groups");
        return Promise.resolve([]);
      });

      const load = store.load();
      // Both reads are dispatched before either resolves.
      expect(order).toEqual(["hosts", "groups"]);
      await load;
    });

    it("shares one in-flight load between concurrent callers", async () => {
      const store = useHostsStore();

      await Promise.all([store.load(), store.load(), store.load()]);

      expect(api.listHosts).toHaveBeenCalledTimes(1);
      expect(api.listGroups).toHaveBeenCalledTimes(1);
      expect(store.hosts).toHaveLength(2);
      expect(store.groups).toHaveLength(1);
    });

    it("starts a fresh read once the previous one has settled", async () => {
      const store = useHostsStore();

      await store.load();
      await store.load();

      expect(api.listHosts).toHaveBeenCalledTimes(2);
    });
  });

  describe("addHost", () => {
    it("adds a host and updates the store", async () => {
      const store = useHostsStore();
      const newHost: Host = {
        id: "temp",
        label: "New Server",
        hostname: "new.com",
        port: 22,
        username: "user",
        group_id: null,
        key_id: null,
        auth: "publickey",
        tags: [],
        startup_command: null,
        proxy_command: null,
        jump_host_id: null,
      };
      const saved = await store.addHost(newHost);

      expect(saved.id).toBe("new-host");
      expect(store.hosts).toContainEqual(saved);
    });
  });

  describe("updateHost", () => {
    it("updates an existing host in the store", async () => {
      const store = useHostsStore();
      await store.load();

      const updated = { ...store.hosts[0], label: "Updated Server" };
      const result = await store.updateHost(updated);

      expect(result.label).toBe("Updated Server");
      expect(store.hosts[0].label).toBe("Updated Server");
    });
  });

  describe("deleteHost", () => {
    it("removes a host from the store", async () => {
      const store = useHostsStore();
      await store.load();
      expect(store.hosts).toHaveLength(2);

      await store.deleteHost("host-1");
      expect(store.hosts).toHaveLength(1);
      expect(store.hosts.find((h) => h.id === "host-1")).toBeUndefined();
    });
  });

  describe("addGroup", () => {
    it("adds a group and updates the store", async () => {
      const store = useHostsStore();
      const group = await store.addGroup("Staging");

      expect(group.name).toBe("Staging");
      expect(store.groups).toContainEqual(group);
    });
  });

  describe("deleteGroup", () => {
    it("removes a group and unassigns hosts", async () => {
      const store = useHostsStore();
      await store.load();

      await store.deleteGroup("group-1");
      expect(store.groups).toHaveLength(0);
      // host-2 had group_id = "group-1", should now be null
      const host2 = store.hosts.find((h) => h.id === "host-2");
      expect(host2?.group_id).toBeNull();
    });
  });

  describe("filteredHosts", () => {
    it("filters by search query across label, hostname, username, and tags", async () => {
      const store = useHostsStore();
      await store.load();

      // Search by label
      store.searchQuery = "Server";
      expect(store.filteredHosts).toHaveLength(1);
      expect(store.filteredHosts[0].label).toBe("My Server");

      // Search by hostname
      store.searchQuery = "db.internal";
      expect(store.filteredHosts).toHaveLength(1);
      expect(store.filteredHosts[0].hostname).toBe("db.internal");

      // Search by username
      store.searchQuery = "admin";
      expect(store.filteredHosts).toHaveLength(1);
      expect(store.filteredHosts[0].username).toBe("admin");

      // Search by tag
      store.searchQuery = "prod";
      expect(store.filteredHosts).toHaveLength(1);
      expect(store.filteredHosts[0].tags).toContain("prod");

      // No match
      store.searchQuery = "nonexistent";
      expect(store.filteredHosts).toHaveLength(0);

      // Empty query shows all
      store.searchQuery = "";
      expect(store.filteredHosts).toHaveLength(2);
    });

    it("filters by selected group", async () => {
      const store = useHostsStore();
      await store.load();

      store.selectedGroupId = "group-1";
      expect(store.filteredHosts).toHaveLength(1);
      expect(store.filteredHosts[0].id).toBe("host-2");

      store.selectedGroupId = null;
      expect(store.filteredHosts).toHaveLength(2);
    });

    it("combines group filter and search query", async () => {
      const store = useHostsStore();
      await store.load();

      store.selectedGroupId = "group-1";
      store.searchQuery = "Database";
      expect(store.filteredHosts).toHaveLength(1);

      store.searchQuery = "nonexistent";
      expect(store.filteredHosts).toHaveLength(0);
    });
  });
});
