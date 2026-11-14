import { nextTick, onBeforeUnmount, watch, type Ref } from "vue";

// Keyboard focus for a modal panel: it starts inside, Tab and Shift+Tab cycle
// within it, Escape closes it, and focus goes back to whatever opened it.
//
// Keys are taken at the document rather than on the panel. A focused control
// that becomes disabled (a Save button while it saves) drops focus to <body>,
// and a panel-level listener would never see the next Tab. Only the topmost
// modal answers, so a confirmation over a form owns the keys until it closes.
// Topmost is the highest layer on screen and, among modals in the same layer,
// the one opened last.

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), ' +
  'select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

interface OpenModal {
  token: symbol;
  panel: Ref<HTMLElement | null>;
}

const openModals: OpenModal[] = [];

// The layer a panel is drawn in: the z-index of its outermost ancestor that has
// one, which is the overlay it sits in. Read from the page's own styles so the
// stylesheet stays the only place that orders the overlays.
function layerOf(element: HTMLElement | null): number {
  let layer = 0;
  for (let node = element; node && node !== document.body; node = node.parentElement) {
    const zIndex = Number.parseInt(getComputedStyle(node).zIndex, 10);
    if (!Number.isNaN(zIndex)) layer = zIndex;
  }
  return layer;
}

function topModal(): OpenModal | undefined {
  let top: OpenModal | undefined;
  let topLayer = -Infinity;
  for (const modal of openModals) {
    const layer = layerOf(modal.panel.value);
    if (layer >= topLayer) {
      top = modal;
      topLayer = layer;
    }
  }
  return top;
}

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

  // A native modal <dialog> sits in the top layer and makes the rest of the page
  // inert. A panel behind it cannot take focus, so handling Tab here would only
  // swallow the key. The dialog answers Tab and Escape itself.
  function behindNativeDialog(): boolean {
    return Array.from(document.querySelectorAll("dialog[open]")).some(
      (dialog) => !dialog.contains(panel.value),
    );
  }

  function onKeydown(event: KeyboardEvent) {
    // An input method owns its keys while it composes: Escape cancels the
    // composition, and answering it here would also close the dialog and lose
    // what was typed. WebKit reports the key that ends a composition as 229.
    if (event.isComposing || event.keyCode === 229) return;
    if (topModal()?.token !== token || behindNativeDialog()) return;
    if (event.key === "Escape") {
      event.preventDefault();
      // Immediate, because every open modal listens on the document. Once this
      // one closes, the modal beneath it is topmost, and a browser runs the
      // microtasks that unregister this one between two listeners of the same
      // event, so the next listener would answer the same Escape too.
      event.stopImmediatePropagation();
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
    // The panel itself holds focus while every control in it is disabled; it
    // is outside the control list, so it counts as a boundary both ways.
    const inside = !!focused && focused !== element && element.contains(focused);
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
    openModals.push({ token, panel });
    document.addEventListener("keydown", onKeydown, true);
  }

  function close() {
    if (!active) return;
    active = false;
    document.removeEventListener("keydown", onKeydown, true);
    const index = openModals.findIndex((modal) => modal.token === token);
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
