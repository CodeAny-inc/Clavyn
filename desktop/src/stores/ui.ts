import { defineStore } from "pinia";
import { ref } from "vue";

export const useUiStore = defineStore("ui", () => {
  // All concurrent SSH attempts share one modal and one settlement. Replacing a
  // resolver strands earlier panes in their busy state, including on cancel.
  const showVaultUnlockModal = ref(false);
  let vaultUnlockResolve: ((success: boolean) => void) | null = null;
  let vaultUnlockPending: Promise<boolean> | null = null;

  // Global focus owner, shared with terminal autofocus guards.
  const commandPaletteOpen = ref(false);

  // Fullscreen pane
  const fullscreenPaneId = ref<string | null>(null);

  // Sidebar collapsed state (icon-only mode for small screens)
  const sidebarCollapsed = ref(false);

  // Mobile sidebar overlay (for very small screens)
  const mobileSidebarOpen = ref(false);

  function requestVaultUnlock(): Promise<boolean> {
    if (vaultUnlockPending) return vaultUnlockPending;
    vaultUnlockPending = new Promise(resolve => { vaultUnlockResolve = resolve; });
    showVaultUnlockModal.value = true;
    return vaultUnlockPending;
  }

  function resolveVaultUnlock(success: boolean) {
    const resolve = vaultUnlockResolve;
    // Clear before settling: a continuation may immediately request a new cycle.
    vaultUnlockResolve = null;
    vaultUnlockPending = null;
    showVaultUnlockModal.value = false;
    resolve?.(success);
  }

  function toggleFullscreen(paneId: string) {
    if (fullscreenPaneId.value === paneId) {
      fullscreenPaneId.value = null;
    } else {
      fullscreenPaneId.value = paneId;
    }
  }

  function exitFullscreen() {
    fullscreenPaneId.value = null;
  }

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value;
  }

  function closeMobileSidebar() {
    mobileSidebarOpen.value = false;
  }

  function openMobileSidebar() {
    mobileSidebarOpen.value = true;
  }

  return {
    showVaultUnlockModal,
    commandPaletteOpen,
    fullscreenPaneId,
    sidebarCollapsed,
    mobileSidebarOpen,
    requestVaultUnlock,
    resolveVaultUnlock,
    toggleFullscreen,
    exitFullscreen,
    toggleSidebar,
    closeMobileSidebar,
    openMobileSidebar,
  };
});
