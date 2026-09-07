import { vi, beforeAll, afterEach } from "vitest";
import "@testing-library/jest-dom/vitest";

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

const defaultInvoke = vi.fn(async (cmd: string, args?: any) => {
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

vi.mock("@tauri-apps/api/core", () => ({
  invoke: defaultInvoke,
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

// --- Cleanup after each test ---
afterEach(() => {
  clearInvokeHandlers();
  mockListeners.clear();
  defaultInvoke.mockClear();
});

// Re-export for convenience
export { vi };
