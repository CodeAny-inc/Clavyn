import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useIdentitiesStore } from "./identities";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import type { Identity } from "../types";

beforeEach(() => setActivePinia(createPinia()));
describe("identity cache readiness", () => {
  it("shares an in-flight load rather than treating the initial empty array as ready", async () => {
    let complete!: (items: Identity[]) => void;
    setInvokeHandler("list_identities", () => new Promise<Identity[]>(resolve => { complete = resolve; }));
    const store = useIdentitiesStore();
    const first = store.ensureLoaded();
    const second = store.ensureLoaded();
    expect(store.loaded).toBe(false);
    expect(store.loading).toBe(true);
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "list_identities")).toHaveLength(1);
    complete([]);
    await Promise.all([first, second]);
    expect(store.loaded).toBe(true);
    expect(store.loading).toBe(false);
    await store.ensureLoaded();
    expect(getInvokeMock().mock.calls.filter(([name]) => name === "list_identities")).toHaveLength(1);
  });
  it("fails closed after a rejected load and permits an explicit retry", async () => {
    const store = useIdentitiesStore();
    setInvokeHandler("list_identities", () => { throw new Error("fixture failure"); });
    await expect(store.ensureLoaded()).rejects.toThrow("Could not load SSH identities");
    expect(store.loaded).toBe(false);
    expect(store.loading).toBe(false);
    setInvokeHandler("list_identities", () => []);
    await store.ensureLoaded();
    expect(store.loaded).toBe(true);
    expect(store.loadError).toBe("");
  });
  it("does not mark an invalid backend response as a successfully loaded empty list", async () => {
    setInvokeHandler("list_identities", () => undefined);
    await expect(useIdentitiesStore().ensureLoaded()).rejects.toThrow("Could not load SSH identities");
  });
  it("surfaces refresh failures without rejecting legacy fire-and-forget load callers", async () => {
    const store = useIdentitiesStore();
    await store.ensureLoaded();
    setInvokeHandler("list_identities", () => { throw new Error("fixture failure"); });
    await expect(store.load()).resolves.toBeUndefined();
    expect(store.loaded).toBe(false);
    expect(store.loadError).not.toBe("");
  });
});
