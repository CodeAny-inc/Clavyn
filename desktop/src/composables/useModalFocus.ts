import { nextTick, onBeforeUnmount, watch, type Ref } from "vue";

// Keyboard focus for a modal panel: it starts inside, Tab and Shift+Tab cycle
// within it, Escape closes it, and focus goes back to whatever opened it.
//
// Keys are taken at the document rather than on the panel. A focused control
// that becomes disabled (a Save button while it saves) drops focus to <body>,
// and a panel-level listener would never see the next Tab. Only the most
// recently opened modal answers, so a confirmation over a form owns the keys
// until it closes.

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), ' +
  'select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

const openModals: symbol[] = [];

export function useModalFocus(
  panel: Ref<HTMLElement | null>,
  isOpen: () => boolean,
  onEscape: () => void,
) {
  const token = Symbol("modal");
  let opener: HTMLElement | null = null;
  let active = false;

  function focusables(): HTMLElement[] {
    return Array.from(panel.value?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []).filter(
      (el) => !el.closest("[inert]") && el.getClientRects().length > 0,
    );
  }

  function onKeydown(event: KeyboardEvent) {
    if (openModals[openModals.length - 1] !== token) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onEscape();
      return;
    }
    if (event.key !== "Tab") return;
    const fields = focusables();
    const element = panel.value;
    if (!element) return;
    if (!fields.length) {
      event.preventDefault();
      element.focus();
      return;
    }
    const first = fields[0];
    const last = fields[fields.length - 1];
    const focused = document.activeElement;
    const inside = !!focused && element.contains(focused);
    if (event.shiftKey && (focused === first || !inside)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (focused === last || !inside)) {
      event.preventDefault();
      first.focus();
    }
  }

  function open() {
    if (active) return;
    active = true;
    opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    openModals.push(token);
    document.addEventListener("keydown", onKeydown, true);
  }

  function close() {
    if (!active) return;
    active = false;
    document.removeEventListener("keydown", onKeydown, true);
    const index = openModals.indexOf(token);
    if (index >= 0) openModals.splice(index, 1);
    const target = opener;
    opener = null;
    const focused = document.activeElement;
    // Only when focus is still in the modal or nowhere: never pull it back
    // from somewhere the user has already moved to.
    const unclaimed = !focused || focused === document.body || !!panel.value?.contains(focused);
    if (target?.isConnected && unclaimed) target.focus();
  }

  watch(isOpen, (value) => (value ? open() : close()), { immediate: true });

  // The first field gets focus when the panel appears, unless something inside
  // it has already taken focus (a confirmation focuses its own safe button).
  watch(panel, async (element) => {
    if (!element) return;
    await nextTick();
    if (element.contains(document.activeElement)) return;
    const fields = focusables();
    const first = fields.find((el) => el.matches("input, select, textarea")) ?? fields[0];
    (first ?? element).focus();
  });

  onBeforeUnmount(close);
}
