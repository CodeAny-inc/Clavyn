import { vi, beforeAll, afterEach } from "vitest";
import "@testing-library/jest-dom/vitest";

// Existing component tests assert full endpoint strings (identity resolution,
// readiness, etc.). Address masking is a display-only concern tested in
// isolation, so disable it by default in the test environment.
try {
  const raw = localStorage.getItem("clavyn-settings");
  const parsed = raw ? JSON.parse(raw) : {};
  localStorage.setItem("clavyn-settings", JSON.stringify({ ...parsed, maskAddresses: false }));
} catch {
  // localStorage may be unavailable in some environments; ignore.
}

// --- Mock crypto.randomUUID ---
if (!globalThis.crypto) {
  (globalThis as any).crypto = {};
}
if (!globalThis.crypto.randomUUID) {
  let counter = 0;
  (globalThis.crypto as any).randomUUID = vi.fn(() => {
    counter += 1;
    return `00000000-0000-4000-8000-${String(counter).padStart(12, "0")}`;
  });
}

// --- Mock @tauri-apps/api/core invoke ---
// All Tauri IPC calls go through `invoke`. We provide a configurable mock
// so individual tests can override the return value for specific commands.
const mockInvokeHandlers = new Map<string, (...args: any[]) => any>();

// Output sinks handed to the session-creating commands, so tests can deliver
// terminal output the way the backend does.
interface SessionSink {
  channel: MockChannel<ArrayBuffer>;
  index: number;
  /** Frames on the asynchronous route, waiting for the test to release them. */
  held: (() => void)[];
}
const sessionSinks = new Map<string, SessionSink>();

const defaultInvoke = vi.fn(async (cmd: string, args?: any) => {
  if (args?.onOutput && typeof args.sessionId === "string") {
    sessionSinks.set(args.sessionId, { channel: args.onOutput, index: 0, held: [] });
  }
  if (mockInvokeHandlers.has(cmd)) {
    return mockInvokeHandlers.get(cmd)!(args);
  }
  if (cmd === "list_identities") return [];
  if (cmd === "connect_ssh") return {
    username: args.expectedUsername ?? args.host.username,
    hostname: args.host.hostname,
    port: args.host.port,
  };
  return undefined;
});

// Payload size at or above which the real transport stops evaluating a frame
// inline and fetches it asynchronously instead
// (`MAX_RAW_DIRECT_EXECUTE_THRESHOLD` in Tauri's `ipc/channel.rs`).
const RAW_INLINE_LIMIT = 1024;

// Stand-in for the real IPC channel. It reassembles strictly in order, the way
// `@tauri-apps/api` does: the backend stamps each frame with an index, and a
// frame whose index runs ahead of the next expected one is parked until its
// predecessors have been delivered. That matters because the two delivery
// routes are not equally fast, so a small frame can reach the webview while a
// larger one sent before it is still being fetched.
class MockChannel<T> {
  onmessage: (message: T) => void = () => {};
  private next = 0;
  private parked = new Map<number, T>();
  receive(message: T, index: number) {
    if (index !== this.next) {
      this.parked.set(index, message);
      return;
    }
    this.onmessage(message);
    this.next += 1;
    while (this.parked.has(this.next)) {
      const held = this.parked.get(this.next)!;
      this.parked.delete(this.next);
      this.onmessage(held);
      this.next += 1;
    }
  }
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: defaultInvoke,
  Channel: MockChannel,
}));

// --- Mock @tauri-apps/api/event listen ---
const mockListeners: Map<string, ((event: any) => void)[]> = new Map();

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, handler: (event: any) => void) => {
    if (!mockListeners.has(event)) mockListeners.set(event, []);
    mockListeners.get(event)!.push(handler);
    return () => {
      const arr = mockListeners.get(event);
      if (arr) {
        const idx = arr.indexOf(handler);
        if (idx >= 0) arr.splice(idx, 1);
      }
    };
  }),
  emit: vi.fn(async (event: string, payload?: any) => {
    const arr = mockListeners.get(event);
    if (arr) arr.forEach((h) => h({ event, payload }));
  }),
}));

// --- Mock @tauri-apps/plugin-dialog ---
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

// --- Helper to register invoke handlers ---
export function setInvokeHandler(cmd: string, handler: (...args: any[]) => any) {
  mockInvokeHandlers.set(cmd, handler);
}

export function clearInvokeHandlers() {
  mockInvokeHandlers.clear();
}

export function getInvokeMock() {
  return defaultInvoke;
}

// --- Helper to emit events to listeners ---
export function emitTauriEvent(event: string, payload: any) {
  const arr = mockListeners.get(event);
  if (arr) arr.forEach((h) => h({ event, payload }));
}

// --- Helper to deliver terminal output on a session's sink ---
function sendOnSink(sessionId: string, payload: ArrayBuffer) {
  const sink = sessionSinks.get(sessionId);
  if (!sink) return;
  const index = sink.index++;
  const deliver = () => sink.channel.receive(payload, index);
  // A batch large enough to take the asynchronous route is held until the test
  // releases it, so a test can put a frame in flight and then keep going.
  //
  // Held unconditionally, which is stricter than the browser fixture's opt-in
  // `holdOutput` switch. The two suites need opposite defaults. A unit test
  // drives one session through one interleaving it chose, and the mistake worth
  // catching is asserting against a frame the real transport would still be
  // fetching, so the delay is on by default and a test that wants the batch
  // says so by releasing it. The browser fixture drives whole user flows where
  // a large frame is usually incidental scrollback nothing asserts on, and
  // holding those by default would strand output no test ever releases.
  if (payload.byteLength >= RAW_INLINE_LIMIT) sink.held.push(deliver);
  else deliver();
}

export function emitSessionOutput(sessionId: string, text: string) {
  const bytes = new TextEncoder().encode(text);
  // Copy into a standalone buffer: the sink receives an ArrayBuffer, matching
  // the binary payload the backend sends.
  sendOnSink(sessionId, bytes.slice().buffer);
}

/** Delivers batches held on the asynchronous route, oldest first. */
export function deliverHeldSessionOutput(sessionId: string) {
  sessionSinks.get(sessionId)?.held.splice(0).forEach(deliver => deliver());
}

/**
 * Ends a session the way the backend does: the zero-length end-of-stream frame
 * on the session's own sink, then the `session-closed` event. The frame is
 * small enough to be delivered inline, so it queues behind anything still held
 * on the asynchronous route — while the event, which carries no index, does
 * not.
 */
export function emitSessionClosed(sessionId: string | null, reason = "fixture EOF") {
  if (sessionId) sendOnSink(sessionId, new ArrayBuffer(0));
  emitTauriEvent("session-closed", { session_id: sessionId, reason });
}

// --- Cleanup after each test ---
afterEach(() => {
  clearInvokeHandlers();
  mockListeners.clear();
  sessionSinks.clear();
  defaultInvoke.mockClear();
});

// Re-export for convenience
export { vi };
