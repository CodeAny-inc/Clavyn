// A modal on screen: the app's own dialogs expose `aria-modal`, and a native
// dialog opened with `showModal()` is an open `<dialog>`. The app opens no other
// kind of `<dialog>`, so any open one is modal.
const MODAL = '[aria-modal="true"], dialog[open]';

/**
 * Whether a modal is on screen. While one is, the page behind it must not act
 * on an app-wide shortcut: it would swap the view out from under an open form
 * or open a tab beneath the unlock prompt.
 *
 * Read from the document rather than from any one modal's state, so it covers
 * every kind: the shared dialog, the unlock and update prompts, and the native
 * session picker, whose backdrop can leave focus on the page.
 */
export function isModalOpen(): boolean {
  return document.querySelector(MODAL) !== null;
}
