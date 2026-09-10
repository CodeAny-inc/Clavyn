import type { Terminal } from "@xterm/xterm";

// A browser keeps only a fixed number of WebGL contexts alive per renderer
// process — 16 in Chromium — and silently kills the oldest ones once that is
// passed. A killed context leaves its terminal frozen for the addon's
// three-second restoration window before the DOM renderer takes over, so the
// cap must never be reached rather than merely survived. The workspace keeps
// every pane of every tab mounted at once, so the number of terminals is not
// bounded by what is on screen; this budget bounds the GPU-backed subset to
// half the cap and leaves every other pane on xterm's DOM renderer, which is
// the renderer they would all have used anyway.
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
  const addon = new module.WebglAddon();
  let detached = false;
  const lease: Lease = {
    paneId,
    detach: () => {
      if (detached) return;
      detached = true;
      // Disposing the addon puts xterm back on its DOM renderer with the
      // buffer intact, so giving up a slot costs quality and never contents.
      try {
        addon.dispose();
      } catch {
        // A renderer that fails to unwind must not take the pane with it.
      }
    },
  };
  // A GPU reset, a driver update or the browser reclaiming contexts fires this
  // once the addon gives up on restoration. Without the dispose the pane keeps
  // a dead context and renders nothing at all.
  addon.onContextLoss(() => drop(lease));
  try {
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
