// Address obfuscation for display. The full host address is never rendered when
// masking is enabled. Only a minimal prefix (first 2 characters) of the host
// is kept — just enough for a hint — and the rest is replaced with a
// fixed-length bullet mask. The fixed length is intentional: it prevents
// length-based information leakage so an observer cannot infer how long the
// real hostname or IP is, let alone reconstruct it.
//
// The host label (shown separately) remains the primary identifier, so the
// address only needs to offer a minimal hint, not full disambiguation.

// Fixed-length mask: 12 bullets. Constant regardless of input length so the
// mask never leaks the real address length.
const MASK = "••••••••••••";
const PREFIX_LEN = 2;

/**
 * Mask a bare hostname/IP. Keeps only the first 2 characters (1 for very
 * short names) and replaces everything else with a fixed-length bullet mask.
 * All dots, colons, and subsequent labels/groups are dropped — they would
 * reveal structure (IPv4 vs IPv6 vs hostname, number of labels, etc.).
 */
export function maskHostname(hostname: string): string {
  if (!hostname) return hostname;
  const prefix = hostname.length <= 3 ? hostname.slice(0, 1) : hostname.slice(0, PREFIX_LEN);
  return prefix + MASK;
}

/**
 * Mask a full endpoint string in any of the forms the app renders:
 * `user@host:port`, `host:port`, `[ipv6]:port`, `user@[ipv6]:port`, or a bare
 * host. Status prefixes separated by ` · ` (e.g. "Resolving SSH identity · …")
 * are preserved and only the trailing address portion is masked. The port is
 * preserved (it is a small, non-secret space and helps the user recognize the
 * connection).
 */
export function maskAddress(address: string): string {
  if (!address) return address;

  // Preserve a leading status prefix like "Resolving SSH identity · " and only
  // mask the address portion that follows the separator.
  const sep = " · ";
  const sepIdx = address.lastIndexOf(sep);
  if (sepIdx >= 0) {
    return address.slice(0, sepIdx + sep.length) + maskAddress(address.slice(sepIdx + sep.length));
  }

  // Only mask strings that look like an address. Status labels such as "Local
  // shell" have no user, port, bracket, or dotted/IPv6 host, so they pass
  // through unchanged.
  if (!address.includes("@") && !address.includes(":") && !address.includes(".") && !address.includes("[")) {
    return address;
  }

  let user = "";
  let rest = address;
  const atIdx = address.lastIndexOf("@");
  if (atIdx >= 0) {
    user = address.slice(0, atIdx + 1);
    rest = address.slice(atIdx + 1);
  }

  let host = rest;
  let port = "";
  let bracketed = false;
  if (rest.startsWith("[")) {
    // Bracketed IPv6: [addr]:port
    const close = rest.indexOf("]");
    if (close >= 0) {
      host = rest.slice(1, close);
      port = rest.slice(close + 1);
      bracketed = true;
    }
  } else {
    // Split a trailing numeric port. A bare (unbracketed) IPv6 has no port in
    // the app's rendered strings, so a non-numeric suffix stays with the host.
    const colonIdx = rest.lastIndexOf(":");
    if (colonIdx >= 0 && /^\d+$/.test(rest.slice(colonIdx + 1))) {
      host = rest.slice(0, colonIdx);
      port = rest.slice(colonIdx);
    }
  }

  const maskedHost = maskHostname(host);
  return user + (bracketed ? `[${maskedHost}]` : maskedHost) + port;
}
