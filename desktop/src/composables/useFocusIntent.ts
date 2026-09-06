import { onBeforeUnmount, onMounted } from "vue";
import { useUiStore } from "../stores/ui";
import { useUpdateStore } from "../stores/update";

// One lightweight observer per document, shared by all terminal owners. Only a
// revision is retained: never capture keys, passwords or other input contents.
let users = 0;
let revision = 0;
const events = ["focusin", "pointerdown", "keydown"] as const;
function invalidate() { revision += 1; }

/** Capture before deferring focus; any newer interaction invalidates the ticket. */
export function useFocusIntent() {
  const ui = useUiStore();
  const update = useUpdateStore();
  onMounted(() => {
    if (users++ === 0) events.forEach(event => document.addEventListener(event, invalidate, true));
  });
  onBeforeUnmount(() => {
    if (--users === 0) events.forEach(event => document.removeEventListener(event, invalidate, true));
  });
  function capture() {
    const captured = revision;
    return () => captured === revision;
  }
  function blocked() {
    // Reactive flags also protect the interval before an overlay is rendered.
    return ui.commandPaletteOpen || ui.showVaultUnlockModal || ui.mobileSidebarOpen ||
      (update.showModal && update.available) ||
      !!document.querySelector('dialog[open], [data-terminal-focus-blocker], [role="menu"]');
  }
  return { capture, blocked };
}
