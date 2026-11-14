import { afterEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h, nextTick, ref } from "vue";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import Dialog from "./ui/Dialog.vue";
import UpdateModal from "./UpdateModal.vue";
import VaultUnlockModal from "./VaultUnlockModal.vue";
import { useUpdateStore } from "../stores/update";
import { useUiStore } from "../stores/ui";
import { useVaultStore } from "../stores/vault";
import { getInvokeMock } from "../test/setup";

const RELEASE = {
  available: true,
  version: "9.9.9",
  current_version: "0.1.0",
  date: null,
  body: "Release notes",
};

let wrapper: VueWrapper | undefined;
afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  document.body.innerHTML = "";
});

// The app's modal stack as App.vue mounts it: a form dialog, the unlock prompt
// and the update prompt, each free to open while another is already showing.
function mountApp() {
  const pinia = createPinia();
  setActivePinia(pinia);
  const formOpen = ref(false);
  const formCloses = ref(0);
  const Host = defineComponent({
    setup: () => () => [
      h("button", { id: "opener", onClick: () => { formOpen.value = true; } }, "Add host"),
      h(Dialog, {
        open: formOpen.value,
        title: "Add host",
        onClose: () => { formCloses.value += 1; formOpen.value = false; },
      }, { default: () => [h("input", { id: "label" }), h("input", { id: "hostname" })] }),
      h(UpdateModal),
      h(VaultUnlockModal),
    ],
  });
  wrapper = mount(Host, { attachTo: document.body, global: { plugins: [pinia] } });
  return { formCloses, update: useUpdateStore() };
}

async function openForm() {
  const opener = document.getElementById("opener")!;
  opener.focus();
  opener.click();
  await flushPromises();
  expect(document.activeElement?.id).toBe("label");
}

// An update that arrives while another modal is open: the app shows the prompt
// through the same store call as Ctrl/Cmd+U.
async function showUpdate(update: ReturnType<typeof useUpdateStore>) {
  update.info = RELEASE;
  update.showUpdateDialog();
  await flushPromises();
}

function dialogNamed(name: string): HTMLElement | null {
  const dialogs = Array.from(document.querySelectorAll<HTMLElement>('[role="dialog"]'));
  return dialogs.find((dialog) => {
    const title = document.getElementById(dialog.getAttribute("aria-labelledby") ?? "");
    return title?.textContent?.trim() === name;
  }) ?? null;
}

const updatePrompt = () => dialogNamed("Update Available");
const form = () => dialogNamed("Add host");
const button = (label: string) =>
  Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
    (candidate) => candidate.textContent?.trim() === label || candidate.getAttribute("aria-label") === label,
  )!;

function press(key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  (document.activeElement ?? document.body).dispatchEvent(event);
  return event;
}

describe("UpdateModal over another modal", () => {
  it("is a named modal dialog, so app shortcuts leave it alone", async () => {
    const { update } = mountApp();
    await showUpdate(update);

    const prompt = updatePrompt();
    expect(prompt).not.toBeNull();
    expect(prompt!.getAttribute("aria-modal")).toBe("true");
  });

  it("takes focus without landing on the install action", async () => {
    const { update } = mountApp();
    await openForm();
    await showUpdate(update);

    // An Enter or Space typed into the form a moment earlier must not start an
    // install and restart the app.
    expect(updatePrompt()!.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).toBe(button("Close"));
    expect(document.activeElement).not.toBe(button("Update Now"));
  });

  it("keeps Tab and Shift+Tab inside the prompt instead of the dialog behind it", async () => {
    const { update } = mountApp();
    await openForm();
    await showUpdate(update);

    button("Update Now").focus();
    const forward = press("Tab");
    expect(forward.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(button("Close"));

    const backward = press("Tab", { shiftKey: true });
    expect(backward.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(button("Update Now"));
    expect(form()!.contains(document.activeElement)).toBe(false);
  });

  it("pulls focus out of the dialog behind it when Tab is pressed there", async () => {
    const { update } = mountApp();
    await openForm();
    await showUpdate(update);

    document.getElementById("hostname")!.focus();
    const tab = press("Tab");
    expect(tab.defaultPrevented).toBe(true);
    expect(updatePrompt()!.contains(document.activeElement)).toBe(true);
  });

  it("dismisses the prompt on Escape and leaves the dialog open", async () => {
    const { update, formCloses } = mountApp();
    await openForm();
    await showUpdate(update);

    press("Escape");
    await flushPromises();

    expect(update.showModal).toBe(false);
    expect(updatePrompt()).toBeNull();
    expect(formCloses.value).toBe(0);
    expect(form()).not.toBeNull();
  });

  it("gives Tab and Escape back to the dialog once the prompt closes", async () => {
    const { update, formCloses } = mountApp();
    await openForm();
    await showUpdate(update);
    press("Escape");
    await flushPromises();

    // Focus returns to the field that held it, not to the page behind.
    expect(document.activeElement?.id).toBe("label");

    document.getElementById("hostname")!.focus();
    const tab = press("Tab");
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement?.id).toBe("label");

    press("Escape");
    await flushPromises();
    expect(formCloses.value).toBe(1);
    expect(form()).toBeNull();
  });

  it("takes the keys again when reopened over the dialog", async () => {
    const { update, formCloses } = mountApp();
    await openForm();
    await showUpdate(update);
    press("Escape");
    await flushPromises();

    update.showUpdateDialog();
    await flushPromises();
    expect(updatePrompt()).not.toBeNull();

    press("Escape");
    await flushPromises();
    expect(update.showModal).toBe(false);
    expect(formCloses.value).toBe(0);
    expect(form()).not.toBeNull();
  });

  it("dismisses the prompt before the unlock prompt behind it", async () => {
    const { update } = mountApp();
    const ui = useUiStore();
    vi.spyOn(useVaultStore(), "refreshBiometricState").mockResolvedValue(undefined);
    void ui.requestVaultUnlock().catch(() => {});
    await flushPromises();
    expect(ui.showVaultUnlockModal).toBe(true);

    await showUpdate(update);
    press("Escape");
    await flushPromises();
    expect(update.showModal).toBe(false);
    expect(ui.showVaultUnlockModal).toBe(true);

    press("Escape");
    await flushPromises();
    expect(ui.showVaultUnlockModal).toBe(false);
  });

  it("hides the prompt on Escape during an install without touching the install", async () => {
    const { update } = mountApp();
    await showUpdate(update);
    update.installing = true;
    await nextTick();

    // The close button and a backdrop click already hide the prompt in this
    // state; Escape matches them and the download carries on behind it.
    press("Escape");
    await flushPromises();

    expect(update.showModal).toBe(false);
    expect(update.installing).toBe(true);
    expect(getInvokeMock().mock.calls.map(([command]) => command)).not.toContain("install_update");
  });
});
