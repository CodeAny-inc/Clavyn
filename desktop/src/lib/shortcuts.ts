// App shortcuts that must also work while a terminal has focus.
//
// On macOS they use ⌘, which a terminal program never receives. Elsewhere
// Ctrl+<key> belongs to the shell: Ctrl+F moves forward a character, Ctrl+K
// kills to the end of the line, Ctrl+Left/Right move by word. Those platforms
// therefore use Ctrl+Shift, as GNOME Terminal and Windows Terminal do.

export function isMacPlatform(): boolean {
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  return /mac/i.test(nav.userAgentData?.platform || navigator.platform || "");
}

/** Whether the event carries the terminal-safe app modifier and nothing else. */
export function isTerminalShortcut(event: KeyboardEvent): boolean {
  if (event.altKey) return false;
  return isMacPlatform()
    ? event.metaKey && !event.ctrlKey && !event.shiftKey
    : event.ctrlKey && event.shiftKey && !event.metaKey;
}

/** How to write a terminal-safe shortcut for `key` on this platform. */
export function terminalShortcutLabel(key: string): string {
  return isMacPlatform() ? `⌘${key}` : `Ctrl+Shift+${key}`;
}
