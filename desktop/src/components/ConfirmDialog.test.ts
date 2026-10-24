import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import ConfirmDialog from "./ConfirmDialog.vue";
import { confirmAction, pendingConfirmation } from "../composables/useConfirm";
import { useVaultStore } from "../stores/vault";

const request = {
  title: "Delete key",
  message: 'Delete key "laptop"?',
  confirmLabel: "Delete",
  destructive: true,
};

let wrapper: VueWrapper | undefined;

function dialog() {
  return document.querySelector<HTMLElement>('[role="alertdialog"]');
}

function button(label: string) {
  const found = [...(dialog()?.querySelectorAll("button") ?? [])]
    .find((item) => item.textContent?.trim() === label);
  expect(found, `Missing the "${label}" button`).toBeDefined();
  return found!;
}

async function open(overrides: Partial<typeof request> = {}) {
  const answer = confirmAction({ ...request, ...overrides });
  await flushPromises();
  expect(dialog()).not.toBeNull();
  // Wrapped, so awaiting `open` waits for the dialog rather than the answer.
  return { answer };
}

beforeEach(() => {
  setActivePinia(createPinia());
  document.body.innerHTML = "";
  wrapper = mount(ConfirmDialog, { attachTo: document.body });
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
});

describe("ConfirmDialog", () => {
  it("resolves true when the action is confirmed", async () => {
    const { answer } = await open();
    button("Delete").click();
    await expect(answer).resolves.toBe(true);
    await flushPromises();
    expect(dialog()).toBeNull();
  });

  it("resolves false when cancelled", async () => {
    const { answer } = await open();
    button("Cancel").click();
    await expect(answer).resolves.toBe(false);
  });

  it("resolves false on Escape, without letting the key reach the app", async () => {
    const { answer } = await open();
    let reachedWindow = false;
    const listener = () => { reachedWindow = true; };
    window.addEventListener("keydown", listener);
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    window.removeEventListener("keydown", listener);
    await expect(answer).resolves.toBe(false);
    expect(reachedWindow).toBe(false);
  });

  it("resolves false when the overlay is clicked", async () => {
    const { answer } = await open();
    document.querySelector<HTMLElement>("[data-terminal-focus-blocker]")!.click();
    await expect(answer).resolves.toBe(false);
  });

  it("shows the message and labels the dialog for assistive technology", async () => {
    await open();
    const element = dialog()!;
    expect(element.getAttribute("aria-modal")).toBe("true");
    const title = document.getElementById(element.getAttribute("aria-labelledby")!);
    expect(title?.textContent).toBe("Delete key");
    const description = document.getElementById(element.getAttribute("aria-describedby")!);
    expect(description?.textContent?.trim()).toBe('Delete key "laptop"?');
  });

  it("focuses Cancel for a destructive action and the action otherwise", async () => {
    await open();
    expect(document.activeElement).toBe(button("Cancel"));
    button("Cancel").click();
    await flushPromises();

    await open({ destructive: false, confirmLabel: "Continue" });
    expect(document.activeElement).toBe(button("Continue"));
  });

  it("declines an open confirmation when another one is requested", async () => {
    const { answer: first } = await open();
    const second = confirmAction({ ...request, message: "Second question" });
    await expect(first).resolves.toBe(false);
    expect(pendingConfirmation.value?.message).toBe("Second question");
    await flushPromises();
    button("Delete").click();
    await expect(second).resolves.toBe(true);
  });

  it("declines an open confirmation when the vault locks", async () => {
    const vault = useVaultStore();
    vault.unlocked = true;
    const { answer } = await open();
    vault.unlocked = false;
    await expect(answer).resolves.toBe(false);
    await flushPromises();
    expect(dialog()).toBeNull();
  });
});
