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
const sessionSinks = new Map<string, { onmessage: (message: ArrayBuffer) => void }>();

const defaultInvoke = vi.fn(async (cmd: string, args?: any) => {
  if (args?.onOutput && typeof args.sessionId === "string") {
    sessionSinks.set(args.sessionId, args.onOutput);
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

// Stand-in for the real IPC channel: the application only ever assigns
// `onmessage` and hands the instance to `invoke`, so tests deliver output by
// calling that handler with an ArrayBuffer.
class MockChannel<T> {
  onmessage: (message: T) => void = () => {};
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
export function emitSessionOutput(sessionId: string, text: string) {
  const bytes = new TextEncoder().encode(text);
  // Copy into a standalone buffer: the sink receives an ArrayBuffer, matching
  // the binary payload the backend sends.
  sessionSinks.get(sessionId)?.onmessage(bytes.slice().buffer);
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
