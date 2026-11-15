import { COOLING_DOWN, HOST_CHANGE_DECLINED, hasTag, untagged } from "./confirmation";

// A host or identity write that would let a vault key sign in somewhere new
// is confirmed by the backend in a native dialog. Cancelling it fails the write
// with a tag: an answer rather than a fault, and nothing was saved. For a while
// after a Cancel the dialog does not open again, longer after each further one,
// and a write in that time is refused with a tag of its own that says how long.

/** What to say about waiting, taken from the seconds the backend names. */
function tryAgain(cause: unknown): string {
  const seconds = /for (\d+)s/.exec(String(cause))?.[1];
  return seconds ? `try again in ${seconds}s to be asked.` : "try again shortly to be asked.";
}

function cancelledRecently(cause: unknown): string {
  return `A confirmation was cancelled recently; ${tryAgain(cause)}`;
}

export function isDeclinedHostChange(cause: unknown): boolean {
  return hasTag(cause, HOST_CHANGE_DECLINED);
}

/** What a host or identity form shows when saving did not go through. */
export function saveFailure(cause: unknown): string {
  if (isDeclinedHostChange(cause)) {
    return "Not saved. The change was not allowed in the confirmation dialog.";
  }
  if (hasTag(cause, COOLING_DOWN)) return `Not saved. ${cancelledRecently(cause)}`;
  return `Could not save: ${untagged(cause)}`;
}

/** What to tell the user when deleting an identity failed for a reason other
 * than their own Cancel in the dialog. */
export function deleteFailure(cause: unknown): string {
  if (hasTag(cause, COOLING_DOWN)) return `Not deleted. ${cancelledRecently(cause)}`;
  return `Failed to delete identity: ${untagged(cause)}`;
}
