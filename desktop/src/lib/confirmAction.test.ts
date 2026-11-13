import { afterEach, describe, expect, it, vi } from "vitest";
import { getInvokeMock, setInvokeHandler } from "../test/setup";
import { confirmAction } from "./confirmAction";

describe("confirmAction", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("asks through the dialog plugin's message command with OK and Cancel", async () => {
    setInvokeHandler("plugin:dialog|message", () => "Ok");

    await expect(confirmAction('Delete host "atlas"?')).resolves.toBe(true);
    expect(getInvokeMock()).toHaveBeenCalledWith("plugin:dialog|message", {
      message: 'Delete host "atlas"?',
      kind: "warning",
      buttons: "OkCancel",
    });
  });

  it("answers no when the dialog is cancelled or closed", async () => {
    setInvokeHandler("plugin:dialog|message", () => "Cancel");

    await expect(confirmAction("Delete?")).resolves.toBe(false);
  });

  it("answers no when the dialog cannot be shown", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    setInvokeHandler("plugin:dialog|message", () => {
      throw new Error("dialog.message not allowed");
    });

    await expect(confirmAction("Delete?")).resolves.toBe(false);
    expect(logged).toHaveBeenCalled();
  });

  // In the app `window.confirm` returns a Promise, which is truthy whatever
  // the user answers, so it must never be what decides.
  it("does not consult window.confirm", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    setInvokeHandler("plugin:dialog|message", () => "Cancel");

    await expect(confirmAction("Delete?")).resolves.toBe(false);
    expect(confirm).not.toHaveBeenCalled();
  });
});
