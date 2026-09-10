import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { useTabsStore, isPane, isSplit, type Pane, type PaneTree } from "./tabs";
import type { Host } from "../types";

// Mock api.closeSession so closeTab doesn't try to call Tauri
vi.mock("../api", () => ({
  closeSession: vi.fn(),
  createLocalTerminal: vi.fn(),
  connectSsh: vi.fn(),
  sessionWrite: vi.fn(),
  sessionResize: vi.fn(),
  listSessions: vi.fn(() => Promise.resolve([])),
  sessionOutput: vi.fn(() => ({ onmessage: () => {} })),
  onSessionClosed: vi.fn(() => Promise.resolve(() => {})),
}));

const mockHost: Host = {
  id: "host-1",
  label: "My Server",
  hostname: "example.com",
  port: 22,
  username: "root",
  group_id: null,
  key_id: null,
  auth: "publickey",
  tags: [],
  startup_command: null,
  proxy_command: null,
  jump_host_id: null,
};

describe("tabs store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  describe("newTab", () => {
    it("creates a local terminal tab by default", () => {
      const store = useTabsStore();
      const tab = store.newTab();

      expect(tab.title).toBe("Local");
      expect(store.tabs).toHaveLength(1);
      expect(store.activeTabId).toBe(tab.id);
      expect(isPane(tab.tree)).toBe(true);
      const pane = tab.tree as Pane;
      expect(pane.terminalType).toBe("local");
      expect(pane.hostId).toBeNull();
      expect(pane.title).toBe("Local Terminal");
      expect(pane.connected).toBe(false);
    });

    it("creates an SSH tab when a host is provided", () => {
      const store = useTabsStore();
      const tab = store.newTab(mockHost);

      expect(tab.title).toBe("My Server");
      const pane = tab.tree as Pane;
      expect(pane.terminalType).toBe("ssh");
      expect(pane.hostId).toBe("host-1");
      expect(pane.title).toBe("My Server");
    });
  });

  describe("closeTab", () => {
    it("removes the tab and clears activeTabId if it was active", () => {
      const store = useTabsStore();
      const tab1 = store.newTab();
      const tab2 = store.newTab();

      expect(store.activeTabId).toBe(tab2.id);
      store.closeTab(tab2.id);
      expect(store.tabs).toHaveLength(1);
      expect(store.activeTabId).toBe(tab1.id);
    });

    it("sets activeTabId to null when closing the last tab", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      store.closeTab(tab.id);
      expect(store.tabs).toHaveLength(0);
      expect(store.activeTabId).toBeNull();
    });
  });

  describe("setActiveTab", () => {
    it("switches the active tab", () => {
      const store = useTabsStore();
      const tab1 = store.newTab();
      const tab2 = store.newTab();
      store.setActiveTab(tab1.id);
      expect(store.activeTabId).toBe(tab1.id);
      expect(store.activeTab?.id).toBe(tab1.id);
    });
  });

  describe("splitPane", () => {
    it("splits the active pane horizontally", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane = tab.tree as Pane;

      const newPane = store.splitPane(pane.id, "horizontal");
      expect(newPane).not.toBeNull();
      const tree = store.activeTab!.tree;
      expect(isSplit(tree)).toBe(true);
      const split = tree as any;
      expect(split.direction).toBe("horizontal");
      expect(split.ratio).toBe(0.5);
      expect(isPane(split.first)).toBe(true);
      expect(isPane(split.second)).toBe(true);
      expect(split.first.id).toBe(pane.id);
      expect(split.second.id).toBe(newPane!.id);
    });

    it("splits the active pane vertically with a host", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane = tab.tree as Pane;

      const newPane = store.splitPane(pane.id, "vertical", mockHost);
      expect(newPane).not.toBeNull();
      expect(newPane!.terminalType).toBe("ssh");
      expect(newPane!.hostId).toBe("host-1");
    });

    it("returns null when no active tab", () => {
      const store = useTabsStore();
      const result = store.splitPane("nonexistent", "horizontal");
      expect(result).toBeNull();
    });
  });

  describe("closePane", () => {
    it("closes the tab when closing the only pane", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane = tab.tree as Pane;
      store.closePane(pane.id);
      expect(store.tabs).toHaveLength(0);
    });

    it("removes a pane from a split, leaving the sibling", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane1 = tab.tree as Pane;
      const pane2 = store.splitPane(pane1.id, "horizontal")!;

      store.closePane(pane2.id);
      const tree = store.activeTab!.tree;
      expect(isPane(tree)).toBe(true);
      expect((tree as Pane).id).toBe(pane1.id);
    });

    it("removes first pane from a split, leaving the second", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane1 = tab.tree as Pane;
      const pane2 = store.splitPane(pane1.id, "horizontal")!;

      store.closePane(pane1.id);
      const tree = store.activeTab!.tree;
      expect(isPane(tree)).toBe(true);
      expect((tree as Pane).id).toBe(pane2.id);
    });
  });

  describe("setPaneConnected", () => {
    it("marks a pane as connected with a session ID", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane = tab.tree as Pane;

      store.setPaneConnected(pane.id, "session-123");
      const updated = store.activeTab!.tree as Pane;
      expect(updated.connected).toBe(true);
      expect(updated.sessionId).toBe("session-123");
    });
  });

  describe("setPaneTitle", () => {
    it("updates the pane title", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane = tab.tree as Pane;

      store.setPaneTitle(pane.id, "New Title");
      const updated = store.activeTab!.tree as Pane;
      expect(updated.title).toBe("New Title");
    });
  });

  describe("setRatio", () => {
    it("updates the split ratio", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane = tab.tree as Pane;
      store.splitPane(pane.id, "horizontal");

      const split = store.activeTab!.tree as any;
      store.setRatio(split.id, 0.7);
      const updated = store.activeTab!.tree as any;
      expect(updated.ratio).toBe(0.7);
    });
  });

  describe("firstPane", () => {
    it("returns the first pane of a single-pane tree", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane = tab.tree as Pane;
      const first = store.firstPane(tab.tree);
      expect(first?.id).toBe(pane.id);
    });

    it("returns the first pane of a split tree", () => {
      const store = useTabsStore();
      const tab = store.newTab();
      const pane1 = tab.tree as Pane;
      store.splitPane(pane1.id, "horizontal");
      const first = store.firstPane(store.activeTab!.tree);
      expect(first?.id).toBe(pane1.id);
    });
  });

  describe("isPane / isSplit helpers", () => {
    it("correctly identifies panes and splits", () => {
      const pane: PaneTree = {
        id: "p1",
        sessionId: null,
        hostId: null,
        terminalType: "local",
        title: "Local",
        connected: false,
        closing: false,
      };
      expect(isPane(pane)).toBe(true);
      expect(isSplit(pane)).toBe(false);

      const split: PaneTree = {
        id: "s1",
        direction: "horizontal",
        ratio: 0.5,
        first: pane,
        second: pane,
      };
      expect(isPane(split)).toBe(false);
      expect(isSplit(split)).toBe(true);
    });
  });
});
