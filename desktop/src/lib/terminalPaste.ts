// xterm wraps pasted text in ESC[200~ … ESC[201~ when the remote enables
// bracketed paste, but passes the text through unchanged. Clipboard content
// carrying its own ESC[201~ (or the single-byte C1 CSI, U+009B) would end the
// bracket early, and whatever follows it would run as typed input. Pasted text
// is data, so every C0 and C1 control is removed except tab, line feed and
// carriage return, which multi-line pastes need.
const PASTE_CONTROLS = /[\x00-\x08\x0b\x0c\x0e-\x1f\x80-\x9f]/g;

export function sanitizePaste(text: string): string {
  return text.replace(PASTE_CONTROLS, "");
}
