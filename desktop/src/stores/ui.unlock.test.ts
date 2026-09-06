import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useUiStore } from "./ui";

beforeEach(() => setActivePinia(createPinia()));
describe("shared vault unlock", () => {
  it.each([true, false])("settles all concurrent callers with %s", async success => {
    const ui = useUiStore();
    const waiting = Array.from({ length: 5 }, () => ui.requestVaultUnlock());
    expect(ui.showVaultUnlockModal).toBe(true);
    ui.resolveVaultUnlock(success);
    expect(await Promise.all(waiting)).toEqual(Array(5).fill(success));
    expect(ui.showVaultUnlockModal).toBe(false);
  });
  it("allows a reentrant new cycle without reusing the previous result", async () => {
    const ui = useUiStore();
    const first = ui.requestVaultUnlock();
    const next = first.then(() => ui.requestVaultUnlock());
    ui.resolveVaultUnlock(false);
    await first; await Promise.resolve();
    expect(ui.showVaultUnlockModal).toBe(true);
    ui.resolveVaultUnlock(true);
    expect(await next).toBe(true);
    expect(ui.showVaultUnlockModal).toBe(false);
    ui.resolveVaultUnlock(false); // duplicate settlement is harmless
    expect(ui.showVaultUnlockModal).toBe(false);
  });
});
