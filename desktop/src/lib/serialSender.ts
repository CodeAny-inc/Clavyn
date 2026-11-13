// Tauri runs each command as its own task, so two requests in flight at once
// can reach the backend in either order. A serial sender keeps one request in
// flight at a time for whichever owner (a terminal session) it last sent for.
// Values pushed meanwhile are merged into a single held value, and once the
// request settles `settled` decides what, if anything, goes next.
//
// Held values belong to the owner whose request is in flight. Pushing for a
// different owner starts that owner's request straight away and discards
// them, so a replacement session never inherits them or waits behind the old
// session's request, and the old request then settles without a follow-up.
//
// Values handed to `push` belong to the sender from then on: `merge` may
// update a held value in place.

export type SendFailure = { cause: unknown };

export interface SerialSenderOptions<T> {
  send(owner: string, value: T): Promise<unknown>;
  merge(held: T, value: T): T;
  /**
   * Runs when the request for the current owner settles, with whatever was
   * held behind it and the rejection when it failed. Returns the value to send
   * next for the same owner, or null to send nothing.
   */
  settled(owner: string, held: T | null, failure: SendFailure | null): T | null;
}

export interface SerialSender<T> {
  push(owner: string, value: T): void;
  /** Whether a request for `owner` is in flight. */
  busy(owner: string): boolean;
}

export function createSerialSender<T>(options: SerialSenderOptions<T>): SerialSender<T> {
  let inFlight: string | null = null;
  let held: { value: T } | null = null;

  function push(owner: string, value: T) {
    if (inFlight === owner) {
      held = { value: held ? options.merge(held.value, value) : value };
      return;
    }
    held = null;
    inFlight = owner;
    let request: Promise<unknown>;
    try {
      request = options.send(owner, value);
    } catch (cause) {
      request = Promise.reject(cause);
    }
    void request.then(() => null, (cause: unknown): SendFailure => ({ cause })).then(failure => {
      if (inFlight !== owner) return;
      inFlight = null;
      const waiting = held;
      held = null;
      const next = options.settled(owner, waiting ? waiting.value : null, failure);
      if (next !== null) push(owner, next);
    });
  }

  return { push, busy: owner => inFlight === owner };
}
