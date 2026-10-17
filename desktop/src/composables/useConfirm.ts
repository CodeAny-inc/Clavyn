import { readonly, shallowRef } from "vue";

// The page cannot use `window.confirm`: the dialog plugin replaces it in the
// Tauri webview with an async function, so its Promise is always truthy and
// `if (confirm(...))` proceeds without waiting for an answer. Every
// confirmation goes through this module and the single `ConfirmDialog`
// mounted in `App.vue` instead.

export interface ConfirmRequest {
  title: string;
  message: string;
  confirmLabel: string;
  /** Styles the confirm button as destructive and focuses Cancel first. */
  destructive?: boolean;
}

interface PendingConfirmation extends ConfirmRequest {
  resolve: (answer: boolean) => void;
}

const pending = shallowRef<PendingConfirmation | null>(null);

/** The confirmation on screen, if any. */
export const pendingConfirmation = readonly(pending);

/** Settles the confirmation on screen. Does nothing when none is open. */
export function answerConfirmation(answer: boolean) {
  const current = pending.value;
  if (!current) return;
  pending.value = null;
  current.resolve(answer);
}

/**
 * Asks the user to confirm an action and resolves with their answer.
 *
 * Only one confirmation is shown at a time. A new request declines the one on
 * screen, so the earlier caller never acts on an answer given to a different
 * question.
 */
export function confirmAction(request: ConfirmRequest): Promise<boolean> {
  answerConfirmation(false);
  return new Promise((resolve) => {
    pending.value = { ...request, resolve };
  });
}
