import { describe, expect, it } from "vitest";
import { readableName } from "./readableName";

describe("readableName", () => {
  it("replaces the characters that reorder a name", () => {
    expect(readableName("report\u{202E}fdp.exe")).toBe("report_fdp.exe");
    expect(readableName("a\u{2066}b\u{2069}c\u{200F}.txt")).toBe("a_b_c_.txt");
    expect(
      readableName("x\u{061C}\u{200E}\u{202A}\u{202B}\u{202C}\u{202D}\u{2067}\u{2068}y"),
    ).toBe("x________y");
    expect(readableName("invoice.pdf\u{2028}\u{2029}.exe")).toBe("invoice.pdf__.exe");
    expect(readableName("name\u{7}with\u{1B}controls")).toBe("name_with_controls");
  });

  it("collapses padding that would push the real extension out of view", () => {
    expect(readableName(`invoice.pdf${" ".repeat(80)}.exe`)).toBe("invoice.pdf .exe");
    expect(readableName(`invoice.pdf${"\u{3164}".repeat(40)}.exe`)).toBe("invoice.pdf .exe");
    expect(readableName("invoice.pdf\u{2800}\u{A0}\u{3000}.exe")).toBe("invoice.pdf .exe");
    expect(readableName("invoice.pdf \u{200B} \u{2060} \u{FEFF} \u{200D} .exe")).toBe(
      "invoice.pdf .exe",
    );
  });

  it("keeps names that already read as what they are", () => {
    for (const name of [
      "meeting notes.txt",
      "\u{8CC7}\u{6599}\u{3000}2024.pdf",
      "\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}.txt",
      "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}.png",
      "\u{2764}\u{FE0F} notes.txt",
      "C:x.bat",
      "aux.txt",
    ]) {
      expect(readableName(name)).toBe(name);
    }
  });
});
