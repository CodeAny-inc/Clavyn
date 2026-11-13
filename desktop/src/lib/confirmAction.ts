import { invoke } from "@tauri-apps/api/core";

/**
 * Asks an OK/Cancel question in a native dialog and resolves to whether the
 * user pressed OK.
 *
 * `window.confirm` cannot do this in the app. The dialog plugin replaces it
 * with an async function, whose Promise reads as `true` in an `if`, and that
 * function calls `plugin:dialog|confirm`, a command the plugin does not
 * register. This makes the call the plugin's own `confirm()` makes: `message`
 * with OK/Cancel buttons, which answers `"Ok"` or `"Cancel"`.
 *
 * A dialog that cannot be shown counts as Cancel, so the action it guards does
 * not run. These questions guard against a misclick, not against page script:
 * anything that needs a real control is confirmed natively by the backend,
 * where the page cannot answer for the user.
 */
export async function confirmAction(message: string): Promise<boolean> {
  try {
    const answer = await invoke<string>("plugin:dialog|message", {
      message,
      kind: "warning",
      buttons: "OkCancel",
    });
    return answer === "Ok";
  } catch (error) {
    console.error("The confirmation dialog could not be shown:", error);
    return false;
  }
}
