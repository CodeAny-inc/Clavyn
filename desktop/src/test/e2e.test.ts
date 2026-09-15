/**
 * End-to-end integration tests that simulate the full app workflow:
 * 1. Vault setup → 2. Key generation → 3. Host creation → 4. Terminal connection
 *
 * These tests mock the Tauri IPC layer but exercise the real Pinia stores
 * and their interactions, verifying the complete user journey.
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { useVaultStore } from "../stores/vault";
import { useKeysStore } from "../stores/keys";
import { useHostsStore } from "../stores/hosts";
import { useTabsStore } from "../stores/tabs";
import type { Host, KeyMeta, HostGroup } from "../types";

// --- Mock data ---
const mockKey: KeyMeta = {
  id: "key-1",
  label: "My Key",
  key_type: "ed25519",
  fingerprint: "SHA256:abc123",
  public_key_base64: "ssh-ed25519 AAAA...",
};

const mockGroup: HostGroup = {
  id: "group-1",
  name: "Production",
  color: null,
};

const mockHost: Host = {
  id: "host-1",
  label: "Web Server",
  hostname: "192.168.1.100",
  port: 22,
  username: "deploy",
  group_id: "group-1",
  key_id: "key-1",
  auth: "publickey",
  tags: ["prod", "web"],
  startup_command: null,
  proxy_command: null,
  jump_host_id: null,
};

// --- Mock api ---
vi.mock("../api", () => ({
  // Vault
  vaultIsInitialized: vi.fn(() => Promise.resolve(false)),
  isVaultUnlocked: vi.fn(() => Promise.resolve(false)),
  initializeVault: vi.fn(() => Promise.resolve(true)),
  unlockVault: vi.fn(() => Promise.resolve()),
  lockVault: vi.fn(() => Promise.resolve()),
  // Biometric
  biometricAvailable: vi.fn(() => Promise.resolve(false)),
  biometricPassphraseStored: vi.fn(() => Promise.resolve(false)),
  storeBiometricPassphrase: vi.fn(() => Promise.resolve()),
  unlockWithBiometric: vi.fn(() => Promise.resolve(true)),
  clearBiometricPassphrase: vi.fn(() => Promise.resolve()),
  // Keys
  listKeys: vi.fn(() => Promise.resolve([])),
  generateKey: vi.fn((label: string) =>
    Promise.resolve({ ...mockKey, label }),
  ),
  importKey: vi.fn((label: string) =>
    Promise.resolve({ ...mockKey, id: "imported-1", label }),
  ),
  deleteKey: vi.fn(() => Promise.resolve()),
  // Hosts
  listHosts: vi.fn(() => Promise.resolve([])),
  listGroups: vi.fn(() => Promise.resolve([])),
  addHost: vi.fn((host: Host) => Promise.resolve({ ...host, id: "host-new" })),
  updateHost: vi.fn((host: Host) => Promise.resolve(host)),
  deleteHost: vi.fn(() => Promise.resolve()),
  addGroup: vi.fn((name: string) =>
    Promise.resolve({ id: "group-new", name, color: null }),
  ),
  deleteGroup: vi.fn(() => Promise.resolve()),
  // Sessions
  closeSession: vi.fn(() => Promise.resolve()),
  createLocalTerminal: vi.fn(() => Promise.resolve()),
  connectSsh: vi.fn(() => Promise.resolve()),
  sessionWrite: vi.fn(() => Promise.resolve()),
  sessionResize: vi.fn(() => Promise.resolve()),
  listSessions: vi.fn(() => Promise.resolve([])),
  sessionOutput: vi.fn(() => ({ onmessage: () => {} })),
  onSessionClosed: vi.fn(() => Promise.resolve(() => {})),
}));

import * as api from "../api";

describe("E2E: Full app workflow", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  describe("Vault setup → Key generation → Host creation → Terminal", () => {
    it("completes the full user journey", async () => {
      // --- Step 1: Vault setup ---
      const vault = useVaultStore();

      // Check initial status
      await vault.checkStatus();
      expect(vault.needsSetup).toBe(true);

      // Initialize vault
      await vault.initialize("strong-passphrase-123");
      expect(vault.initialized).toBe(true);
      expect(vault.unlocked).toBe(true);
      expect(api.initializeVault).toHaveBeenCalledWith("strong-passphrase-123");

      // --- Step 2: Key generation ---
      const keys = useKeysStore();
      await keys.load();
      expect(keys.keys).toHaveLength(0);

      // Generate a key
      const key = await keys.generateKey("Deploy Key");
      expect(key.id).toBe("key-1");
      expect(key.label).toBe("Deploy Key");
      expect(keys.keys).toHaveLength(1);

      // --- Step 3: Host creation ---
      const hosts = useHostsStore();
      await hosts.load();
      expect(hosts.hosts).toHaveLength(0);

      // Create a group
      const group = await hosts.addGroup("Production");
      expect(group.name).toBe("Production");

      // Create a host
      const newHost: Host = {
        id: "temp",
        label: "Web Server",
        hostname: "192.168.1.100",
        port: 22,
        username: "deploy",
        group_id: group.id,
        key_id: key.id,
        auth: "publickey",
        tags: ["prod", "web"],
        startup_command: null,
        proxy_command: null,
        jump_host_id: null,
      };
      const savedHost = await hosts.addHost(newHost);
      expect(savedHost.id).toBe("host-new");
      expect(hosts.hosts).toHaveLength(1);

      // Search for the host
      hosts.searchQuery = "web";
      expect(hosts.filteredHosts).toHaveLength(1);
      hosts.searchQuery = "";

      // --- Step 4: Terminal creation ---
      const tabs = useTabsStore();

      // Create a local terminal tab
      const localTab = tabs.newTab();
      expect(localTab.title).toBe("Local");
      const localPane = localTab.tree as any;
      expect(localPane.terminalType).toBe("local");

      // Simulate connecting the local terminal
      tabs.setPaneConnected(localPane.id, "session-local-1");
      const connectedPane = tabs.activeTab!.tree as any;
      expect(connectedPane.connected).toBe(true);
      expect(connectedPane.sessionId).toBe("session-local-1");

      // Create an SSH terminal tab for the host
      const sshTab = tabs.newTab(savedHost);
      expect(sshTab.title).toBe("Web Server");
      const sshPane = sshTab.tree as any;
      expect(sshPane.terminalType).toBe("ssh");
      expect(sshPane.hostId).toBe("host-new");

      // Simulate connecting the SSH session
      tabs.setPaneConnected(sshPane.id, "session-ssh-1");
      const connectedSshPane = tabs.activeTab!.tree as any;
      expect(connectedSshPane.connected).toBe(true);

      // Split the terminal
      const splitPane = tabs.splitPane(sshPane.id, "horizontal");
      expect(splitPane).not.toBeNull();
      expect(splitPane!.terminalType).toBe("local"); // new pane is local by default

      // Verify the tree structure
      const tree = tabs.activeTab!.tree;
      expect("direction" in tree).toBe(true);
      const split = tree as any;
      expect(split.direction).toBe("horizontal");
      expect(split.ratio).toBe(0.5);

      // Close the split pane
      tabs.closePane(splitPane!.id);
      const afterClose = tabs.activeTab!.tree;
      expect("direction" in afterClose).toBe(false); // back to single pane

      // --- Cleanup: Lock vault ---
      await vault.lock();
      expect(vault.unlocked).toBe(false);
    });
  });

  describe("Key import workflow", () => {
    it("imports a key with a passphrase", async () => {
      const keys = useKeysStore();
      await keys.load();

      const imported = await keys.importKey(
        "Imported RSA Key",
        "-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n-----END OPENSSH PRIVATE KEY-----",
        "my-passphrase",
      );

      expect(imported.id).toBe("imported-1");
      expect(imported.label).toBe("Imported RSA Key");
      expect(keys.keys).toHaveLength(1);
    });
  });

  describe("Host management workflow", () => {
    it("creates, updates, and deletes hosts", async () => {
      const hosts = useHostsStore();

      // Setup: load with existing host
      vi.mocked(api.listHosts).mockResolvedValue([mockHost]);
      vi.mocked(api.listGroups).mockResolvedValue([mockGroup]);
      await hosts.load();

      expect(hosts.hosts).toHaveLength(1);
      expect(hosts.groups).toHaveLength(1);

      // Filter by group
      hosts.selectedGroupId = "group-1";
      expect(hosts.filteredHosts).toHaveLength(1);
      hosts.selectedGroupId = null;

      // Update host
      const updated = { ...mockHost, label: "Updated Server" };
      vi.mocked(api.updateHost).mockResolvedValue(updated);
      const result = await hosts.updateHost(updated);
      expect(result.label).toBe("Updated Server");
      expect(hosts.hosts[0].label).toBe("Updated Server");

      // Delete host
      await hosts.deleteHost("host-1");
      expect(hosts.hosts).toHaveLength(0);
    });
  });

  describe("Multi-tab terminal management", () => {
    it("manages multiple tabs with splits", async () => {
      const tabs = useTabsStore();

      // Create 3 tabs
      const tab1 = tabs.newTab();
      const tab2 = tabs.newTab();
      const tab3 = tabs.newTab();

      expect(tabs.tabs).toHaveLength(3);
      expect(tabs.activeTabId).toBe(tab3.id);

      // Switch to tab1
      tabs.setActiveTab(tab1.id);
      expect(tabs.activeTab?.id).toBe(tab1.id);

      // Split tab1
      const pane1 = tab1.tree as any;
      const pane2 = tabs.splitPane(pane1.id, "vertical");
      expect(pane2).not.toBeNull();

      // Split again (nested)
      const pane3 = tabs.splitPane(pane2!.id, "horizontal");
      expect(pane3).not.toBeNull();

      // Close a pane
      tabs.closePane(pane3!.id);
      // tab1 should still exist with 2 panes
      expect(tabs.tabs.find((t) => t.id === tab1.id)).toBeDefined();

      // Close tab2
      tabs.closeTab(tab2.id);
      expect(tabs.tabs).toHaveLength(2);
      expect(tabs.tabs.find((t) => t.id === tab2.id)).toBeUndefined();
    });
  });

  describe("Vault lock/unlock cycle", () => {
    it("supports locking and re-unlocking", async () => {
      const vault = useVaultStore();

      // Initialize
      await vault.initialize("passphrase");
      expect(vault.unlocked).toBe(true);

      // Lock
      await vault.lock();
      expect(vault.unlocked).toBe(false);
      expect(vault.needsUnlock).toBe(true);

      // Unlock again
      await vault.unlock("passphrase");
      expect(vault.unlocked).toBe(true);
      expect(vault.needsUnlock).toBe(false);
    });
  });
});