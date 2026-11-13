import { describe, expect, it } from "vitest";
import { flushPromises } from "@vue/test-utils";
import { createSerialSender, type SendFailure } from "./serialSender";

function harness(settled: (owner: string, held: string | null, failure: SendFailure | null) => string | null = (_owner, held) => held) {
  const sent: Array<[string, string]> = [];
  const pending: Array<{ resolve: () => void; reject: (cause: unknown) => void }> = [];
  const settles: Array<[string, string | null, unknown]> = [];
  const sender = createSerialSender<string>({
    send: (owner, value) => {
      sent.push([owner, value]);
      return new Promise<void>((resolve, reject) => { pending.push({ resolve, reject }); });
    },
    merge: (held, value) => held + value,
    settled: (owner, held, failure) => {
      settles.push([owner, held, failure?.cause]);
      return settled(owner, held, failure);
    },
  });
  return { sender, sent, pending, settles };
}

describe("createSerialSender", () => {
  it("keeps one request in flight and sends what arrived meanwhile as one request", async () => {
    const { sender, sent, pending } = harness();
    sender.push("a", "1");
    sender.push("a", "2");
    sender.push("a", "3");
    expect(sent).toEqual([["a", "1"]]);
    expect(sender.busy("a")).toBe(true);
    pending[0].resolve();
    await flushPromises();
    expect(sent).toEqual([["a", "1"], ["a", "23"]]);
    pending[1].resolve();
    await flushPromises();
    expect(sent).toHaveLength(2);
    expect(sender.busy("a")).toBe(false);
  });

  it("lets settled drop or replace what was held, and reports the failure", async () => {
    const { sender, sent, pending, settles } = harness((_owner, held, failure) => failure ? null : held);
    sender.push("a", "1");
    sender.push("a", "2");
    pending[0].reject("boom");
    await flushPromises();
    expect(settles).toEqual([["a", "2", "boom"]]);
    expect(sent).toEqual([["a", "1"]]);
    sender.push("a", "3");
    expect(sent).toEqual([["a", "1"], ["a", "3"]]);
  });

  it("starts a new owner at once, without the old owner's held values or a follow-up for the old owner", async () => {
    const { sender, sent, pending, settles } = harness();
    sender.push("old", "1");
    sender.push("old", "2");
    sender.push("new", "x");
    expect(sent).toEqual([["old", "1"], ["new", "x"]]);
    expect(sender.busy("old")).toBe(false);
    pending[0].resolve();
    await flushPromises();
    expect(settles).toEqual([]);
    pending[1].resolve();
    await flushPromises();
    expect(settles).toEqual([["new", null, undefined]]);
    expect(sent).toHaveLength(2);
  });

  it("treats a send that throws as a failed request", async () => {
    const settles: Array<[string, unknown]> = [];
    const sender = createSerialSender<string>({
      send: () => { throw new Error("no transport"); },
      merge: (held, value) => held + value,
      settled: (owner, _held, failure) => { settles.push([owner, failure?.cause]); return null; },
    });
    sender.push("a", "1");
    await flushPromises();
    expect(settles).toEqual([["a", new Error("no transport")]]);
    expect(sender.busy("a")).toBe(false);
  });
});
