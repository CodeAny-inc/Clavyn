import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { OUTPUT_ACK_DELAY_MS, OUTPUT_ACK_THRESHOLD, sessionOutput } from "./api";
import { getInvokeMock, setInvokeHandler } from "./test/setup";

const acks = () =>
  getInvokeMock().mock.calls
    .filter(([command]) => command === "session_output_ack")
    .map(([, args]) => args);

const frame = (length: number) => new Uint8Array(length).fill(120).buffer;

function open(sessionId = "s1") {
  const onData = vi.fn();
  const onEnd = vi.fn();
  const channel = sessionOutput(sessionId, onData, onEnd);
  const deliver = (length: number) => channel.onmessage(frame(length));
  return { onData, onEnd, deliver };
}

describe("session output acknowledgement", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("coalesces small batches into one acknowledgement after a short delay", () => {
    const { onData, deliver } = open();
    deliver(10);
    deliver(2000);
    deliver(5);
    expect(onData).toHaveBeenCalledTimes(3);
    expect(acks()).toEqual([]);

    vi.advanceTimersByTime(OUTPUT_ACK_DELAY_MS);
    expect(acks()).toEqual([{ sessionId: "s1", bytes: 2015 }]);

    // Nothing pending, so the timer does not fire an empty acknowledgement.
    vi.advanceTimersByTime(OUTPUT_ACK_DELAY_MS * 10);
    expect(acks()).toHaveLength(1);
  });

  it("acknowledges at once when the pending total reaches the threshold", () => {
    const { deliver } = open();
    deliver(OUTPUT_ACK_THRESHOLD - 1);
    expect(acks()).toEqual([]);
    deliver(64 * 1024);
    expect(acks()).toEqual([{ sessionId: "s1", bytes: OUTPUT_ACK_THRESHOLD - 1 + 64 * 1024 }]);

    // The delayed acknowledgement was folded into that one.
    vi.advanceTimersByTime(OUTPUT_ACK_DELAY_MS * 10);
    expect(acks()).toHaveLength(1);

    // Later output starts a fresh count.
    deliver(7);
    vi.advanceTimersByTime(OUTPUT_ACK_DELAY_MS);
    expect(acks()).toEqual([
      { sessionId: "s1", bytes: OUTPUT_ACK_THRESHOLD - 1 + 64 * 1024 },
      { sessionId: "s1", bytes: 7 },
    ]);
  });

  it("retries an acknowledgement that failed, so the reader is not left waiting", async () => {
    let fail = true;
    setInvokeHandler("session_output_ack", () => {
      if (fail) throw new Error("ipc unavailable");
    });
    const { deliver } = open();
    deliver(OUTPUT_ACK_THRESHOLD);
    await vi.advanceTimersByTimeAsync(0);
    expect(acks()).toEqual([{ sessionId: "s1", bytes: OUTPUT_ACK_THRESHOLD }]);

    // No further output arrives, so only a retry can hand the credit back.
    fail = false;
    await vi.advanceTimersByTimeAsync(2000);
    expect(acks()).toEqual([
      { sessionId: "s1", bytes: OUTPUT_ACK_THRESHOLD },
      { sessionId: "s1", bytes: OUTPUT_ACK_THRESHOLD },
    ]);
  });

  it("stops retrying once the stream has ended", async () => {
    setInvokeHandler("session_output_ack", () => {
      throw new Error("session closed");
    });
    const { deliver } = open();
    deliver(OUTPUT_ACK_THRESHOLD);
    deliver(0);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(acks()).toHaveLength(1);
  });

  it("acknowledges each session's output under its own id", () => {
    const first = open("first");
    const second = open("second");
    first.deliver(3);
    second.deliver(4);
    vi.advanceTimersByTime(OUTPUT_ACK_DELAY_MS);
    expect(acks()).toEqual(
      expect.arrayContaining([
        { sessionId: "first", bytes: 3 },
        { sessionId: "second", bytes: 4 },
      ]),
    );
    expect(acks()).toHaveLength(2);
  });

  it("never acknowledges the end-of-stream frame or anything after it", () => {
    const { onData, onEnd, deliver } = open();
    deliver(100);
    deliver(0);
    expect(onEnd).toHaveBeenCalledTimes(1);
    expect(onData).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(OUTPUT_ACK_DELAY_MS * 10);
    expect(acks()).toEqual([]);
  });

  it("swallows a failed acknowledgement", async () => {
    setInvokeHandler("session_output_ack", () => Promise.reject(new Error("backend gone")));
    const { onData, deliver } = open();
    deliver(OUTPUT_ACK_THRESHOLD);
    expect(acks()).toHaveLength(1);
    // Let the rejection settle; an unhandled one fails the run.
    await vi.runAllTimersAsync();
    deliver(1);
    expect(onData).toHaveBeenCalledTimes(2);
  });
});
