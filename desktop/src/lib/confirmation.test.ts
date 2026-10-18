import { describe, expect, it } from "vitest";
import { COOLING_DOWN, HOST_CHANGE_DECLINED, HOST_KEY_DECLINED, hasTag, untagged } from "./confirmation";
import { deleteFailure, isDeclinedHostChange, saveFailure } from "./hostChange";

const cooling = `${COOLING_DOWN} a host change confirmation was declined; no further host change confirmations will be offered for 4s`;

describe("confirmation tags", () => {
  it("tells one kind of refusal from another", () => {
    const declined = `${HOST_CHANGE_DECLINED} Allow a vault key to be used: cancelled`;
    expect(hasTag(declined, HOST_CHANGE_DECLINED)).toBe(true);
    expect(hasTag(declined, HOST_KEY_DECLINED)).toBe(false);
    expect(hasTag(cooling, COOLING_DOWN)).toBe(true);
    expect(isDeclinedHostChange(cooling)).toBe(false);
  });

  it("strips the tags from text shown to the user", () => {
    expect(untagged(cooling)).toBe(
      "a host change confirmation was declined; no further host change confirmations will be offered for 4s",
    );
    expect(untagged("store.json was not written")).toBe("store.json was not written");
  });
});

describe("host change failures", () => {
  it("asks for a retry shortly after a recent Cancel instead of reporting a failure", () => {
    expect(saveFailure(cooling)).toBe(
      "Not saved. A confirmation was cancelled a moment ago; try again in a few seconds to be asked.",
    );
    expect(deleteFailure(cooling)).toMatch(/^Not deleted\. A confirmation was cancelled a moment ago/);
  });

  it("reports anything else as a failure", () => {
    expect(saveFailure("store.json was not written")).toBe("Could not save: store.json was not written");
    expect(deleteFailure("store.json was not written")).toBe("Failed to delete identity: store.json was not written");
  });
});
