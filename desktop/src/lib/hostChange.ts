// A host or identity write that would let a vault key sign in somewhere new
// is confirmed by the backend in a native dialog. Cancelling it fails the write
// with this tag: an answer rather than a fault, and nothing was saved.
const DECLINED = "[host-change-confirmation-declined]";

export function isDeclinedHostChange(cause: unknown): boolean {
  return String(cause).includes(DECLINED);
}

/** What a host or identity form shows when saving did not go through. */
export function saveFailure(cause: unknown): string {
  return isDeclinedHostChange(cause)
    ? "Not saved. The change was not allowed in the confirmation dialog."
    : `Could not save: ${cause}`;
}
