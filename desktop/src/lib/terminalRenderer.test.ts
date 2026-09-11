import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Terminal } from "@xterm/xterm";

// One fake addon per instantiation, so a test can assert which pane's renderer
// was torn down and can fire a context loss at a specific one.
class FakeWebglAddon {
  static instances: FakeWebglAddon[] = [];
  static failActivation = false;
  // The real addon's constructor throws before xterm ever sees it when its own
  // `webgl2` probe comes back null — "Webgl2 is only supported on Safari 16 and
  // above" is the message it uses.
  static failConstruction = false;
  disposed = 0;
  loaded = false;
  private handlers: (() => void)[] = [];
  constructor() {
    if (FakeWebglAddon.failConstruction) {
      throw new Error("Webgl2 is only supported on Safari 16 and above");
    }
    FakeWebglAddon.instances.push(this);
  }
  onContextLoss(handler: () => void) { this.handlers.push(handler); return { dispose() {} }; }
  loseContext() { this.handlers.forEach(handler => handler()); }
  dispose() { this.disposed += 1; }
}

vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: FakeWebglAddon }));

function fakeTerminal(): Terminal {
  return {
    loadAddon(addon: FakeWebglAddon) {
      if (FakeWebglAddon.failActivation) throw new Error("WebGL2 not supported");
      addon.loaded = true;
    },
  } as unknown as Terminal;
}

type Module = typeof import("./terminalRenderer");

// The budget is process-wide state, so every test gets a fresh module.
async function load(webgl2 = true): Promise<Module> {
  vi.resetModules();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(((kind: string) =>
    kind === "webgl2" && webgl2 ? { getExtension: () => null } : null) as never);
  return import("./terminalRenderer");
}

beforeEach(() => {
  FakeWebglAddon.instances = [];
  FakeWebglAddon.failActivation = false;
  FakeWebglAddon.failConstruction = false;
});
afterEach(() => { vi.restoreAllMocks(); });

describe("terminal GPU renderer budget", () => {
  it("puts a pane on the GPU renderer when WebGL2 is available", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    await expect(acquireWebglRenderer("pane-1", fakeTerminal())).resolves.toBe(true);
    expect(FakeWebglAddon.instances).toHaveLength(1);
    expect(FakeWebglAddon.instances[0].loaded).toBe(true);
    expect(webglBackedPaneIds()).toEqual(["pane-1"]);
  });

  it("leaves the pane on the DOM renderer when WebGL2 is unavailable", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load(false);
    await expect(acquireWebglRenderer("pane-1", fakeTerminal())).resolves.toBe(false);
    expect(FakeWebglAddon.instances).toHaveLength(0);
    expect(webglBackedPaneIds()).toEqual([]);
  });

  it("probes WebGL2 support once however many panes ask", async () => {
    const { acquireWebglRenderer } = await load();
    await acquireWebglRenderer("pane-1", fakeTerminal());
    await acquireWebglRenderer("pane-2", fakeTerminal());
    await acquireWebglRenderer("pane-3", fakeTerminal());
    const probes = vi.mocked(HTMLCanvasElement.prototype.getContext).mock.calls
      .filter(([kind]) => kind === "webgl2");
    expect(probes).toHaveLength(1);
  });

  it("keeps the pane usable when the renderer refuses to activate", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    FakeWebglAddon.failActivation = true;
    await expect(acquireWebglRenderer("pane-1", fakeTerminal())).resolves.toBe(false);
    expect(webglBackedPaneIds()).toEqual([]);
    expect(FakeWebglAddon.instances[0].disposed).toBe(1);
  });

  // The probe in supportsWebgl2 uses default context attributes and caches its
  // answer, so a platform the addon rejects outright — Safari below 16 — still
  // gets as far as `new WebglAddon()`. That throw has to land as a DOM-renderer
  // fallback, not as a rejected promise nobody is waiting on.
  it("falls back to the DOM renderer when the addon constructor rejects the platform", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    FakeWebglAddon.failConstruction = true;
    await expect(acquireWebglRenderer("pane-1", fakeTerminal())).resolves.toBe(false);
    expect(webglBackedPaneIds()).toEqual([]);
    expect(FakeWebglAddon.instances).toHaveLength(0);
  });

  it("leaves no pending claim behind when the constructor rejects the platform", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    FakeWebglAddon.failConstruction = true;
    await expect(acquireWebglRenderer("pane-1", fakeTerminal())).resolves.toBe(false);
    // A later attempt must be able to try again rather than join a dead claim.
    FakeWebglAddon.failConstruction = false;
    await expect(acquireWebglRenderer("pane-1", fakeTerminal())).resolves.toBe(true);
    expect(webglBackedPaneIds()).toEqual(["pane-1"]);
  });

  it("re-acquiring a pane refreshes its position instead of attaching twice", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    const terminal = fakeTerminal();
    await acquireWebglRenderer("pane-1", terminal);
    await acquireWebglRenderer("pane-2", fakeTerminal());
    await expect(acquireWebglRenderer("pane-1", terminal)).resolves.toBe(true);
    expect(FakeWebglAddon.instances).toHaveLength(2);
    expect(webglBackedPaneIds()).toEqual(["pane-2", "pane-1"]);
  });

  it("holds the budget and evicts the least recently used pane", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds, WEBGL_PANE_BUDGET } = await load();
    for (let i = 0; i < WEBGL_PANE_BUDGET; i++) await acquireWebglRenderer(`pane-${i}`, fakeTerminal());
    expect(webglBackedPaneIds()).toHaveLength(WEBGL_PANE_BUDGET);
    // Bring the oldest pane back to the front, so the next one past it goes instead.
    await acquireWebglRenderer("pane-0", fakeTerminal());
    await acquireWebglRenderer("overflow", fakeTerminal());
    const backed = webglBackedPaneIds();
    expect(backed).toHaveLength(WEBGL_PANE_BUDGET);
    expect(backed).not.toContain("pane-1");
    expect(backed).toContain("pane-0");
    expect(backed[backed.length - 1]).toBe("overflow");
    // The evicted pane's renderer is torn down; it falls back to the DOM renderer.
    expect(FakeWebglAddon.instances[1].disposed).toBe(1);
  });

  it("never exceeds the budget however many panes are mounted", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds, WEBGL_PANE_BUDGET } = await load();
    for (let i = 0; i < 40; i++) await acquireWebglRenderer(`pane-${i}`, fakeTerminal());
    expect(webglBackedPaneIds()).toHaveLength(WEBGL_PANE_BUDGET);
    const live = FakeWebglAddon.instances.filter(addon => addon.disposed === 0);
    expect(live).toHaveLength(WEBGL_PANE_BUDGET);
  });

  it("drops the pane's renderer on context loss without throwing", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    await acquireWebglRenderer("pane-1", fakeTerminal());
    await acquireWebglRenderer("pane-2", fakeTerminal());
    expect(() => FakeWebglAddon.instances[0].loseContext()).not.toThrow();
    expect(FakeWebglAddon.instances[0].disposed).toBe(1);
    expect(webglBackedPaneIds()).toEqual(["pane-2"]);
  });

  it("survives a renderer that throws while unwinding a lost context", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    await acquireWebglRenderer("pane-1", fakeTerminal());
    const addon = FakeWebglAddon.instances[0];
    addon.dispose = () => { throw new Error("gl teardown failed"); };
    expect(() => addon.loseContext()).not.toThrow();
    expect(webglBackedPaneIds()).toEqual([]);
  });

  it("frees the slot again once the lost pane comes back to the front", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    await acquireWebglRenderer("pane-1", fakeTerminal());
    FakeWebglAddon.instances[0].loseContext();
    await acquireWebglRenderer("pane-1", fakeTerminal());
    expect(webglBackedPaneIds()).toEqual(["pane-1"]);
    expect(FakeWebglAddon.instances).toHaveLength(2);
  });

  it("releases the slot when a pane goes away", async () => {
    const { acquireWebglRenderer, releaseWebglRenderer, webglBackedPaneIds } = await load();
    await acquireWebglRenderer("pane-1", fakeTerminal());
    await acquireWebglRenderer("pane-2", fakeTerminal());
    releaseWebglRenderer("pane-1");
    expect(FakeWebglAddon.instances[0].disposed).toBe(1);
    expect(webglBackedPaneIds()).toEqual(["pane-2"]);
    expect(() => releaseWebglRenderer("pane-1")).not.toThrow();
  });

  it("gives one pane one context however often it is shown while loading", async () => {
    const { acquireWebglRenderer, webglBackedPaneIds } = await load();
    const terminal = fakeTerminal();
    const claims = [
      acquireWebglRenderer("pane-1", terminal),
      acquireWebglRenderer("pane-1", terminal),
      acquireWebglRenderer("pane-1", terminal),
    ];
    await expect(Promise.all(claims)).resolves.toEqual([true, true, true]);
    expect(FakeWebglAddon.instances).toHaveLength(1);
    expect(webglBackedPaneIds()).toEqual(["pane-1"]);
  });

  it("does not attach to a pane released while the renderer was still loading", async () => {
    const { acquireWebglRenderer, releaseWebglRenderer, webglBackedPaneIds } = await load();
    const attaching = acquireWebglRenderer("pane-1", fakeTerminal());
    releaseWebglRenderer("pane-1");
    await expect(attaching).resolves.toBe(false);
    expect(webglBackedPaneIds()).toEqual([]);
    expect(FakeWebglAddon.instances).toHaveLength(0);
  });
});
