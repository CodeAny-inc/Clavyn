import { describe, expect, it } from "vitest";
import { maskAddress, maskHostname } from "./maskAddress";

describe("maskHostname", () => {
  it("hides the last octet of an IPv4 address", () => {
    expect(maskHostname("192.168.1.42")).toBe("192.168.1.••••••");
  });

  it("keeps the first three groups of an IPv6 address", () => {
    expect(maskHostname("2001:db8::1")).toBe("2001:db8::••••••");
  });

  it("masks the middle labels of a multi-part hostname", () => {
    expect(maskHostname("atlas.example.test")).toBe("atlas.••••••.test");
  });

  it("masks the second label of a two-part hostname", () => {
    expect(maskHostname("orion.test")).toBe("orion.••••••");
  });

  it("masks the tail of a single-label hostname", () => {
    expect(maskHostname("webserver")).toBe("webse••••••");
  });

  it("fully masks very short single labels", () => {
    expect(maskHostname("db")).toBe("••••••");
  });

  it("passes empty input through unchanged", () => {
    expect(maskHostname("")).toBe("");
  });
});

describe("maskAddress", () => {
  it("masks a user@host:port endpoint", () => {
    expect(maskAddress("deploy@atlas.example.test:22")).toBe("deploy@atlas.••••••.test:22");
  });

  it("masks a bare host:port", () => {
    expect(maskAddress("atlas.example.test:22")).toBe("atlas.••••••.test:22");
  });

  it("masks an IPv4 endpoint while keeping the port", () => {
    expect(maskAddress("root@10.0.0.5:2222")).toBe("root@10.0.0.••••••:2222");
  });

  it("masks a bracketed IPv6 endpoint", () => {
    expect(maskAddress("root@[2001:db8::1]:22")).toBe("root@[2001:db8::••••••]:22");
  });

  it("preserves a status prefix and masks only the trailing address", () => {
    expect(maskAddress("Resolving SSH identity · atlas.example.test:22"))
      .toBe("Resolving SSH identity · atlas.••••••.test:22");
  });

  it("preserves a missing-identity status prefix", () => {
    expect(maskAddress("Missing SSH identity · server.example.test:22"))
      .toBe("Missing SSH identity · server.••••••.test:22");
  });

  it("passes non-address status strings through unchanged", () => {
    expect(maskAddress("Local shell")).toBe("Local shell");
    expect(maskAddress("SSH identity not yet resolved")).toBe("SSH identity not yet resolved");
  });

  it("passes empty input through unchanged", () => {
    expect(maskAddress("")).toBe("");
  });
});
