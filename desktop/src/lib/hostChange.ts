import { COOLING_DOWN, HOST_CHANGE_DECLINED, hasTag, untagged } from "./confirmation";

// A host or identity write that would let a vault key sign in somewhere new
// is confirmed by the backend in a native dialog. Cancelling it fails the write
// with a tag: an answer rather than a fault, and nothing was saved. For a few
// seconds after a Cancel the dialog does not open again, and a write in that
// time is refused with a tag of its own.

const COOLING_DOWN_NOTICE =
  "A confirmation was cancelled a moment ago; try again in a few seconds to be asked.";

export function isDeclinedHostChange(cause: unknown): boolean {
  return hasTag(cause, HOST_CHANGE_DECLINED);
}

/** What a host or identity form shows when saving did not go through. */
export function saveFailure(cause: unknown): string {
  if (isDeclinedHostChange(cause)) {
    return "Not saved. The change was not allowed in the confirmation dialog.";
  }
  if (hasTag(cause, COOLING_DOWN)) return `Not saved. ${COOLING_DOWN_NOTICE}`;
  return `Could not save: ${untagged(cause)}`;
}

/** What to tell the user when deleting an identity failed for a reason other
 * than their own Cancel in the dialog. */
export function deleteFailure(cause: unknown): string {
  if (hasTag(cause, COOLING_DOWN)) return `Not deleted. ${COOLING_DOWN_NOTICE}`;
  return `Failed to delete identity: ${untagged(cause)}`;
}
