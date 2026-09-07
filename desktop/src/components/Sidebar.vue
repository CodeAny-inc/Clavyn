<script setup lang="ts">
import { useVaultStore } from "../stores/vault";
import { useTabsStore } from "../stores/tabs";

const vault = useVaultStore();
const tabs = useTabsStore();

defineProps<{ activeView: string }>();
const emit = defineEmits<{ navigate: [string] }>();

const navItems = [
  { id: "hosts", label: "Hosts", icon: "🖥" },
  { id: "terminal", label: "Terminal", icon: "▶" },
  { id: "keys", label: "Keys", icon: "🔑" },
  { id: "workspaces", label: "Workspaces", icon: "📁" },
  { id: "known-hosts", label: "Known Hosts", icon: "🔒" },
  { id: "vault", label: "Vault", icon: "🛡" },
];
</script>

<template>
  <aside class="sidebar">
    <div class="sidebar-header">
      <h1>Clavyn</h1>
    </div>
    <nav class="sidebar-nav">
      <button
        v-for="item in navItems"
        :key="item.id"
        class="nav-btn"
        :class="{ active: activeView === item.id }"
        @click="emit('navigate', item.id)"
      >
        <span>{{ item.icon }}</span>
        <span>{{ item.label }}</span>
        <span v-if="item.id === 'terminal' && tabs.tabs.length" style="margin-left: auto; font-size: 10px; background: var(--panel-3); padding: 1px 6px; border-radius: 8px;">
          {{ tabs.tabs.length }}
        </span>
      </button>
    </nav>
    <div class="sidebar-footer">
      <div class="status-dot" :class="{ ok: vault.unlocked }"></div>
      <span>{{ vault.unlocked ? "Vault unlocked" : "Vault locked" }}</span>
    </div>
  </aside>
</template>
