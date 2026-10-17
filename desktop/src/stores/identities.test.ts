import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { useIdentitiesStore } from "./identities";
import { useHostsStore } from "./hosts";
import type { Identity } from "../types";

const mockIdentity: Identity = {
  id: "id-1",
  label: "Work Admin",
  username: "admin",
  auth: "publickey",
  key_id: "key-1",
  tags: ["work"],
};

const mockIdentity2: Identity = {
  id: "id-2",
  label: "Personal",
  username: "root",
  auth: "agent",
  key_id: null,
  tags: [],
};

vi.mock("../api", () => ({
  listIdentities: vi.fn(() => Promise.resolve([mockIdentity, mockIdentity2])),
  addIdentity: vi.fn((identity: Identity) =>
    Promise.resolve({ ...identity, id: "new-id" }),
  ),
  updateIdentity: vi.fn((identity: Identity) => Promise.resolve({ ...identity })),
  deleteIdentity: vi.fn(() => Promise.resolve()),
}));

describe("identities store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  describe("load", () => {
    it("loads identities from the API", async () => {
      const store = useIdentitiesStore();
      await store.load();

      expect(store.identities).toHaveLength(2);
      expect(store.identities[0].label).toBe("Work Admin");
      expect(store.identities[1].auth).toBe("agent");
    });
  });

  describe("addIdentity", () => {
    it("adds an identity and updates the store", async () => {
      const store = useIdentitiesStore();
      const newId: Identity = {
        id: "temp",
        label: "New Identity",
        username: "deploy",
        auth: { password: { credential_key: "default" } },
        key_id: null,
        tags: [],
      };
      const saved = await store.addIdentity(newId);

      expect(saved.id).toBe("new-id");
      expect(store.identities).toContainEqual(saved);
    });
  });

  describe("updateIdentity", () => {
    it("updates an existing identity in the store", async () => {
      const store = useIdentitiesStore();
      await store.load();

      const updated = { ...store.identities[0], label: "Updated Admin" };
      const result = await store.updateIdentity(updated);

      expect(result.label).toBe("Updated Admin");
      expect(store.identities[0].label).toBe("Updated Admin");
    });
  });

  describe("deleteIdentity", () => {
    it("removes an identity from the store", async () => {
      const store = useIdentitiesStore();
      await store.load();
      expect(store.identities).toHaveLength(2);

      await store.deleteIdentity("id-1");
      expect(store.identities).toHaveLength(1);
      expect(store.identities.find((i) => i.id === "id-1")).toBeUndefined();
    });

    it("unlinks hosts that used the identity, as the backend does", async () => {
      const store = useIdentitiesStore();
      const hosts = useHostsStore();
      const host = {
        label: "Server", hostname: "example.com", port: 22, username: "root",
        group_id: null, key_id: null, auth: "publickey" as const, tags: [],
      };
      hosts.hosts = [
        { ...host, id: "linked", identity_id: "id-1" },
        { ...host, id: "other", identity_id: "id-2" },
      ];
      await store.load();

      await store.deleteIdentity("id-1");

      expect(hosts.hosts.find((h) => h.id === "linked")?.identity_id).toBeNull();
      expect(hosts.hosts.find((h) => h.id === "other")?.identity_id).toBe("id-2");
    });
  });
});
