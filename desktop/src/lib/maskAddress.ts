// Address obfuscation for display. The full host address is never rendered when
// masking is enabled; enough of the address is kept so a user can still tell
// hosts apart at a glance. The host label (shown separately) remains the
// primary identifier, so the address only needs to disambiguate.

const MASK = "••••••";

/**
 * Mask a bare hostname/IP. Keeps the leading segment(s) that carry the most
 * identifying information and replaces the rest with a bullet placeholder.
 */
export function maskHostname(hostname: string): string {
  if (!hostname) return hostname;

  // IPv4: keep the first three octets, hide the last.
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(hostname)) {
    const parts = hostname.split(".");
    return `${parts[0]}.${parts[1]}.${parts[2]}.${MASK}`;
  }

  // IPv6 (brackets already stripped by maskAddress): keep the first three
  // groups and hide the remainder.
  if (hostname.includes(":")) {
    const groups = hostname.split(":");
    if (groups.length >= 2) return [...groups.slice(0, 3), MASK].join(":");
    return MASK;
  }

  // Dotted hostname: keep the first and last label, hide the middle labels.
  const labels = hostname.split(".");
  if (labels.length >= 3) {
    return [labels[0], MASK, labels[labels.length - 1]].join(".");
  }
  if (labels.length === 2) {
    return [labels[0], MASK].join(".");
  }

  // Single label: keep the first half so short local names stay recognizable.
  if (hostname.length <= 3) return MASK;
  return hostname.slice(0, Math.ceil(hostname.length / 2)) + MASK;
}

/**
 * Mask a full endpoint string in any of the forms the app renders:
 * `user@host:port`, `host:port`, `[ipv6]:port`, `user@[ipv6]:port`, or a bare
 * host. Status prefixes separated by ` · ` (e.g. "Resolving SSH identity · …")
 * are preserved and only the trailing address portion is masked.
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
