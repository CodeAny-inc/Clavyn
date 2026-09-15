import type { Terminal } from "@xterm/xterm";

// A browser keeps only a fixed number of WebGL contexts alive per renderer
// process and kills one of the existing ones once that is passed. A killed
// context leaves its terminal frozen for the addon's three-second restoration
// window before the DOM renderer takes over, so it is worth staying clear of
// the cap rather than merely surviving it. The workspace keeps every pane of
// every tab mounted at once, so the number of terminals is not bounded by what
// is on screen; this budget bounds the GPU-backed subset and leaves every other
// pane on xterm's DOM renderer, which is the renderer they would all have used
// anyway.
//
// What is actually established, and where the number comes from:
//
//   - Chromium (the Windows WebView2 build this was developed against): cap of
//     16, evicting oldest-first. Measured here, and the budget of 16 / 2 = 8 was
//     chosen against it.
//   - WebKit — WKWebView on macOS and WebKitGTK on Linux — is NOT measured. We
//     have no machine for either. WebKit's own source declares the same
//     ceiling: `Source/WebCore/html/canvas/WebGLRenderingContextBase.cpp` has
//     `static constexpr size_t maxActiveContexts = 16` (and 4 for worker
//     contexts), and `addActiveContext` recycles the context with the lowest
//     `activeOrdinal()`, i.e. least-recently-active rather than
//     oldest-created. That is a constant read out of upstream source, not a
//     number observed in this app, and the WebKit revision inside any given
//     WKWebView or WebKitGTK build is not something we pin.
//
// So 8 is half the cap on the one engine we measured and half the documented
// cap on the two we could not. It is deliberately not lowered further: the
// unmeasured engines are the *same* declared number rather than an unknown
// smaller one, and a smaller budget would start demoting panes that are on
// screen in a multi-pane split, which is the case the GPU renderer exists to
// serve. The bet is also cheap to lose — overshooting the real cap costs a few
// seconds of a stale frame on the least recently used pane, not correctness,
// because losing a context falls back to the DOM renderer with the buffer
// intact. Revisit if either WebKit port turns out to enforce a lower ceiling in
// practice than the constant suggests.
export const WEBGL_PANE_BUDGET = 8;

type Lease = { paneId: string; detach: () => void };

// Least-recently-used first. A pane claims a slot when it comes to the front,
// so the budget follows the terminals in use rather than the order panes were
// created in.
const leases: Lease[] = [];
// Claims whose addon module is still in flight, keyed to the terminal the claim
// targets so a pane that unmounts (or is rebuilt) mid-import is not attached.
// A pane can be shown, hidden and shown again inside that window, so repeat
// claims join the pending one instead of racing it into a second context.
const inFlight = new Map<string, { terminal: Terminal; result: Promise<boolean> }>();

let addonModule: Promise<typeof import("@xterm/addon-webgl") | null> | null = null;
let webgl2Supported: boolean | null = null;

// Probing costs a context, so the answer is resolved once and the probe's
// context is handed straight back.
function supportsWebgl2(): boolean {
  if (webgl2Supported === null) {
    try {
      const gl = document.createElement("canvas").getContext("webgl2");
      webgl2Supported = gl !== null;
      gl?.getExtension("WEBGL_lose_context")?.loseContext();
    } catch {
      webgl2Supported = false;
    }
  }
  return webgl2Supported;
}

// Nothing is drawn before a pane is on screen, so the GPU renderer is fetched
// on demand and stays out of the boot-critical chunk.
function loadAddon(): Promise<typeof import("@xterm/addon-webgl") | null> {
  if (!addonModule) {
    addonModule = import("@xterm/addon-webgl").catch(() => {
      addonModule = null;
      return null;
    });
  }
  return addonModule;
}

function drop(lease: Lease): void {
  const at = leases.indexOf(lease);
  if (at >= 0) leases.splice(at, 1);
  lease.detach();
}

function touch(paneId: string): boolean {
  const at = leases.findIndex(lease => lease.paneId === paneId);
  if (at < 0) return false;
  leases.push(leases.splice(at, 1)[0]);
  return true;
}

/**
 * Put `terminal` on the GPU renderer and mark this pane as the most recently
 * used one. Resolves to whether the pane is GPU-backed afterwards; `false`
 * means it keeps xterm's DOM renderer, which is a quality difference and never
 * a functional one. Safe to call repeatedly — a pane that already holds a slot
 * only refreshes its position in the budget.
 *
 * The terminal must already be attached to the document: the addon defers its
 * own activation until then, and a deferred activation would escape the budget.
 */
export function acquireWebglRenderer(paneId: string, terminal: Terminal): Promise<boolean> {
  if (touch(paneId)) return Promise.resolve(true);
  if (!supportsWebgl2()) return Promise.resolve(false);
  const claim = inFlight.get(paneId);
  if (claim?.terminal === terminal) return claim.result;
  const result = attach(paneId, terminal);
  inFlight.set(paneId, { terminal, result });
  return result;
}

async function attach(paneId: string, terminal: Terminal): Promise<boolean> {
  const module = await loadAddon();
  if (inFlight.get(paneId)?.terminal !== terminal) return false;
  inFlight.delete(paneId);
  if (!module) return false;
  // The constructor itself throws, before the addon is ever handed to xterm: it
  // probes for a `webgl2` context with its own attributes (`preserveDrawingBuffer`
  // among them) and raises "Webgl2 is only supported on Safari 16 and above" if
  // that comes back null. `supportsWebgl2` above cannot stand in for it — it
  // probes with default attributes and caches the answer for the life of the
  // process, so a platform that refuses these attributes, or a moment when the
  // engine is already at its context cap, fails here and not there. Uncaught,
  // that rejects the caller's promise instead of falling back to the DOM
  // renderer, which is a hard failure for what is only ever a quality choice.
  let addon: InstanceType<typeof module.WebglAddon>;
  try {
    addon = new module.WebglAddon();
  } catch {
    return false;
  }
  let detached = false;
  const lease: Lease = {
    paneId,
    detach: () => {
      if (detached) return;
      detached = true;
      // Disposing the addon puts xterm back on its DOM renderer with the
      // buffer intact, so giving up a slot costs quality and never contents.
      //
      // That revert is the addon reaching into xterm's private surface:
      // `@xterm/addon-webgl` teardown runs
      // `terminal._core._renderService.setRenderer(terminal._core._createRenderer())`.
      // None of `_core`, `_renderService` or `_createRenderer` appears in
      // `@xterm/xterm`'s public typings, so semver does not protect it — an
      // xterm patch release is free to rename or drop any of them. That is why
      // `@xterm/xterm` and `@xterm/addon-webgl` are both pinned to exact
      // versions in package.json rather than carrying a range; package.json
      // cannot hold the reasoning, so it lives here.
      //
      // The failure mode is what makes an exact pin worth it rather than just a
      // note: the catch below deliberately swallows a broken dispose so it
      // cannot take the pane down with it, which means a renamed private method
      // would fail silently — evicted panes would keep their GPU contexts and
      // the budget above would quietly stop bounding anything. Anyone bumping
      // either package should re-run the eviction and context-loss tests in
      // terminalRenderer.test.ts against the real addon, not just the fake.
      try {
        addon.dispose();
      } catch {
        // A renderer that fails to unwind must not take the pane with it.
      }
    },
  };
  try {
    // A GPU reset, a driver update or the browser reclaiming contexts fires this
    // once the addon gives up on restoration. Without the dispose the pane keeps
    // a dead context and renders nothing at all.
    addon.onContextLoss(() => drop(lease));
    terminal.loadAddon(addon);
  } catch {
    lease.detach();
    return false;
  }
  leases.push(lease);
  while (leases.length > WEBGL_PANE_BUDGET) drop(leases[0]);
  return leases.includes(lease);
}

/** Give up this pane's slot. Must run before the terminal itself is disposed. */
export function releaseWebglRenderer(paneId: string): void {
  inFlight.delete(paneId);
  const at = leases.findIndex(lease => lease.paneId === paneId);
  if (at >= 0) drop(leases[at]);
}

/** Panes currently holding a slot, least recently used first. */
export function webglBackedPaneIds(): string[] {
  return leases.map(lease => lease.paneId);
}
