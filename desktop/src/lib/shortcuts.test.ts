import { afterEach, describe, expect, it, vi } from "vitest";
import { isTerminalShortcut, terminalShortcutLabel } from "./shortcuts";

function key(init: KeyboardEventInit) {
  return new KeyboardEvent("keydown", { key: "f", ...init });
}

describe("terminal-safe shortcuts", () => {
  afterEach(() => vi.restoreAllMocks());

  it("uses ⌘ alone on macOS", () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    expect(isTerminalShortcut(key({ metaKey: true }))).toBe(true);
    expect(isTerminalShortcut(key({ ctrlKey: true }))).toBe(false);
    expect(isTerminalShortcut(key({ metaKey: true, shiftKey: true }))).toBe(false);
    expect(terminalShortcutLabel("F")).toBe("⌘F");
  });

  it("uses Ctrl+Shift elsewhere, leaving plain Ctrl to the shell", () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("Win32");
    expect(isTerminalShortcut(key({ ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(isTerminalShortcut(key({ ctrlKey: true }))).toBe(false);
    expect(isTerminalShortcut(key({ ctrlKey: true, shiftKey: true, altKey: true }))).toBe(false);
    expect(isTerminalShortcut(key({ metaKey: true }))).toBe(false);
    expect(terminalShortcutLabel("F")).toBe("Ctrl+Shift+F");
  });
});
