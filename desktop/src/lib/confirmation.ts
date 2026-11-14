// The backend asks before some commands in a native dialog the page cannot
// answer, and tags the error a refusal raises so a view can tell an answer from
// a fault. The tags are set in `host_key_prompt.rs` and `host_change_prompt.rs`.

/** The user cancelled a host key dialog. */
export const HOST_KEY_DECLINED = "[host-key-confirmation-declined]";

/** The user cancelled a host or identity change dialog. */
export const HOST_CHANGE_DECLINED = "[host-change-confirmation-declined]";

/** Nothing was asked: a recent Cancel still holds that kind of dialog shut. */
export const COOLING_DOWN = "[confirmation-cooling-down]";

const TAGS = [HOST_KEY_DECLINED, HOST_CHANGE_DECLINED, COOLING_DOWN];

export function hasTag(cause: unknown, tag: string): boolean {
  return String(cause).includes(tag);
}

/** The error's text without its tags, for showing to the user. */
export function untagged(cause: unknown): string {
  return TAGS.reduce((text, tag) => text.replace(tag, ""), String(cause)).trim();
}
