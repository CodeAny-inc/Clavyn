import { afterEach, describe, expect, it } from "vitest";
import { defineComponent, h, ref } from "vue";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { useModalFocus } from "./useModalFocus";

// A modal with two buttons, drawn in `layer` when one is given, the way the
// app's overlays carry their z-index.
const Modal = defineComponent({
  props: {
    name: { type: String, required: true },
    open: { type: Boolean, default: true },
    layer: { type: Number, default: undefined },
  },
  emits: ["escape"],
  setup(props, { emit }) {
    const panel = ref<HTMLElement | null>(null);
    useModalFocus(panel, () => props.open, () => emit("escape"));
    return () =>
      props.open
        ? h(
            "div",
            { style: props.layer === undefined ? undefined : { position: "fixed", zIndex: String(props.layer) } },
            h("div", { ref: panel, tabindex: -1 }, [
              h("button", { id: `${props.name}-first` }, "first"),
              h("button", { id: `${props.name}-last` }, "last"),
            ]),
          )
        : null;
  },
});

const wrappers: VueWrapper[] = [];

function modal(name: string, options: { layer?: number; into?: HTMLElement } = {}) {
  const wrapper = mount(Modal, {
    props: { name, layer: options.layer },
    attachTo: options.into ?? document.body,
  });
  wrappers.push(wrapper);
  return wrapper;
}

function nativeDialog() {
  const dialog = document.createElement("dialog");
  dialog.setAttribute("open", "");
  document.body.appendChild(dialog);
  return dialog;
}

function press(key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  document.body.dispatchEvent(event);
  return event;
}

const escapes = (wrapper: VueWrapper) => wrapper.emitted("escape")?.length ?? 0;
const focused = () => document.activeElement?.id;

afterEach(() => {
  wrappers.splice(0).forEach((wrapper) => wrapper.unmount());
  document.body.innerHTML = "";
});

describe("useModalFocus key ownership", () => {
  it("gives the keys to the modal opened last when modals share a layer", async () => {
    const first = modal("first");
    await flushPromises();
    const second = modal("second");
    await flushPromises();

    press("Escape");
    expect(escapes(second)).toBe(1);
    expect(escapes(first)).toBe(0);

    await second.setProps({ open: false });
    press("Escape");
    expect(escapes(first)).toBe(1);
  });

  it("gives the keys to the modal in the higher layer even though it opened first", async () => {
    const above = modal("above", { layer: 110 });
    await flushPromises();
    const below = modal("below", { layer: 50 });
    await flushPromises();

    press("Escape");
    expect(escapes(above)).toBe(1);
    expect(escapes(below)).toBe(0);

    // Tab wraps inside the modal that is on top, not the one hidden under it.
    document.getElementById("above-last")!.focus();
    const tab = press("Tab");
    expect(tab.defaultPrevented).toBe(true);
    expect(focused()).toBe("above-first");

    await above.setProps({ open: false });
    press("Escape");
    expect(escapes(below)).toBe(1);
  });

  // Every open modal listens on the document, so a handled Escape must not
  // reach the next listener: by then the modal that answered has unregistered,
  // and the one beneath it would be topmost and answer the same key.
  it("does not pass an Escape it answered on to a later listener on the document", async () => {
    modal("only");
    await flushPromises();
    let seenLater = 0;
    const listener = () => { seenLater += 1; };
    document.addEventListener("keydown", listener, true);
    try {
      press("Escape");
    } finally {
      document.removeEventListener("keydown", listener, true);
    }
    expect(seenLater).toBe(0);
  });

  it("lets a later modal in a higher layer take the keys over an earlier one", async () => {
    const lower = modal("lower", { layer: 50 });
    await flushPromises();
    const higher = modal("higher", { layer: 110 });
    await flushPromises();

    press("Escape");
    expect(escapes(higher)).toBe(1);
    expect(escapes(lower)).toBe(0);
  });
});

describe("useModalFocus beside a native modal dialog", () => {
  it("leaves Tab and Escape to an open native dialog", async () => {
    const dialog = nativeDialog();
    const form = modal("form");
    await flushPromises();
    document.getElementById("form-last")!.focus();

    const tab = press("Tab");
    expect(tab.defaultPrevented).toBe(false);
    expect(focused()).toBe("form-last");
    const escape = press("Escape");
    expect(escape.defaultPrevented).toBe(false);
    expect(escapes(form)).toBe(0);

    dialog.remove();
    press("Escape");
    expect(escapes(form)).toBe(1);
  });

  it("still answers when its panel is inside the open dialog", async () => {
    const dialog = nativeDialog();
    const inner = modal("inner", { into: dialog });
    await flushPromises();

    press("Escape");
    expect(escapes(inner)).toBe(1);
  });
});

describe("useModalFocus while an input method composes", () => {
  it("leaves Escape to the composition instead of closing the modal", async () => {
    const form = modal("form");
    await flushPromises();

    const escape = press("Escape", { isComposing: true });
    expect(escape.defaultPrevented).toBe(false);
    expect(escapes(form)).toBe(0);
  });

  it("leaves the key that WebKit reports as 229 to the composition", async () => {
    const form = modal("form");
    await flushPromises();

    const escape = press("Escape", { keyCode: 229 });
    expect(escape.defaultPrevented).toBe(false);
    expect(escapes(form)).toBe(0);
  });

  it("leaves Tab to the composition too, instead of wrapping focus", async () => {
    modal("form");
    await flushPromises();
    document.getElementById("form-last")!.focus();

    const tab = press("Tab", { isComposing: true });
    expect(tab.defaultPrevented).toBe(false);
    expect(focused()).toBe("form-last");
  });

  it("answers an ordinary Escape once the composition has ended", async () => {
    const form = modal("form");
    await flushPromises();

    press("Escape", { isComposing: true });
    expect(escapes(form)).toBe(0);
    press("Escape");
    expect(escapes(form)).toBe(1);
  });
});
