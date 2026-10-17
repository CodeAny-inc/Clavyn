import { describe, expect, it } from "vitest";
import { maskAddress, maskHostname, maskHostnamesIn } from "./maskAddress";

const M = "••••••••••••";

describe("maskHostname", () => {
  it("keeps only the first 2 chars of an IPv4 address and drops the rest", () => {
    expect(maskHostname("192.168.1.42")).toBe(`19${M}`);
  });

  it("keeps only the first 2 chars of an IPv6 address and drops the rest", () => {
    expect(maskHostname("2001:db8::1")).toBe(`20${M}`);
  });

  it("keeps only the first 2 chars of a multi-part hostname, dropping all labels", () => {
    expect(maskHostname("atlas.example.test")).toBe(`at${M}`);
  });

  it("keeps only the first 2 chars of a two-part hostname", () => {
    expect(maskHostname("orion.test")).toBe(`or${M}`);
  });

  it("keeps only the first 2 chars of a single-label hostname", () => {
    expect(maskHostname("webserver")).toBe(`we${M}`);
  });

  it("keeps only 1 char for very short (<=3) hostnames", () => {
    expect(maskHostname("db")).toBe(`d${M}`);
    expect(maskHostname("web")).toBe(`w${M}`);
  });

  it("uses a fixed-length mask regardless of input length", () => {
    const short = maskHostname("ab.cd");
    const long = maskHostname("very-long-subdomain.example.production.internal.test");
    expect(short.length).toBe(long.length);
  });

  it("passes empty input through unchanged", () => {
    expect(maskHostname("")).toBe("");
  });
});

describe("maskAddress", () => {
  it("masks a user@host:port endpoint, keeping user and port", () => {
    expect(maskAddress("deploy@atlas.example.test:22")).toBe(`deploy@at${M}:22`);
  });

  it("masks a bare host:port, keeping the port", () => {
    expect(maskAddress("atlas.example.test:22")).toBe(`at${M}:22`);
  });

  it("masks an IPv4 endpoint while keeping the port", () => {
    expect(maskAddress("root@10.0.0.5:2222")).toBe(`root@10${M}:2222`);
  });

  it("masks a bracketed IPv6 endpoint", () => {
    expect(maskAddress("root@[2001:db8::1]:22")).toBe(`root@[20${M}]:22`);
  });

  it("preserves a status prefix and masks only the trailing address", () => {
    expect(maskAddress("Resolving SSH identity · atlas.example.test:22"))
      .toBe(`Resolving SSH identity · at${M}:22`);
  });

  it("preserves a missing-identity status prefix", () => {
    expect(maskAddress("Missing SSH identity · server.example.test:22"))
      .toBe(`Missing SSH identity · se${M}:22`);
  });

  it("passes non-address status strings through unchanged", () => {
    expect(maskAddress("Local shell")).toBe("Local shell");
    expect(maskAddress("SSH identity not yet resolved")).toBe("SSH identity not yet resolved");
  });

  it("passes empty input through unchanged", () => {
    expect(maskAddress("")).toBe("");
  });
});

describe("maskHostnamesIn", () => {
  it("masks a hostname regardless of letter case", () => {
    expect(maskHostnamesIn("server.example.com:22 refused", ["Server.Example.com"]))
      .toBe(`Se${M}:22 refused`);
  });

  it("leaves a longer word that starts with a saved hostname alone", () => {
    expect(maskHostnamesIn("authentication rejected by server", ["auth"]))
      .toBe("authentication rejected by server");
  });

  it("masks a saved hostname followed by a port", () => {
    expect(maskHostnamesIn("auth:22 refused", ["auth"])).toBe(`au${M}:22 refused`);
  });

  it("treats regular-expression characters in a hostname literally", () => {
    expect(maskHostnamesIn("host a+b.test is down", ["a+b.test"]))
      .toBe(`host a+${M} is down`);
  });
});
