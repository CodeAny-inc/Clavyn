// Display form of a name the remote server chose, for the places the user
// reads it before acting on it. The server can put characters in a name that
// change how it reads without changing what it is: a right-to-left override
// makes `report\u{202E}fdp.exe` display as `reportexe.pdf`, and a long run of
// blank characters pushes the real extension out of view. The backend applies
// the same rules to the name it suggests in the save dialog, so the list and
// the dialog agree. Only the display changes; operations keep the raw name.

// Controls, bidirectional controls and the line and paragraph separators.
const REORDERING = /[\p{Cc}\u{061C}\u{200E}\u{200F}\u{2028}-\u{202E}\u{2066}-\u{2069}]/gu;

// Unicode white space, the Hangul fillers (which fonts commonly draw with a
// width) and the braille blank.
const BLANK = /^[\p{White_Space}\u{115F}\u{1160}\u{3164}\u{FFA0}\u{2800}]$/u;

// A run of blank and default-ignorable (invisible) characters.
const GAP = /[\p{White_Space}\p{Default_Ignorable_Code_Point}\u{2800}]+/gu;

/**
 * Collapse padding in one run of blank and invisible characters. Everything
 * from its first blank to its last becomes one space when it holds more than
 * one blank, so interleaving zero-width characters with spaces cannot rebuild
 * the padding. A lone blank is kept, and so are invisible characters outside
 * the blanks, such as the joiners Persian and emoji sequences need.
 */
function collapsePadding(run: string): string {
  const chars = Array.from(run);
  let first = -1;
  let last = -1;
  chars.forEach((c, i) => {
    if (!BLANK.test(c)) return;
    if (first === -1) first = i;
    last = i;
  });
  if (first === last) return run;
  return chars.slice(0, first).join("") + " " + chars.slice(last + 1).join("");
}

/** The name as it should be shown, so it reads as what it is. */
export function readableName(name: string): string {
  return name.replace(REORDERING, "_").replace(GAP, collapsePadding);
}
