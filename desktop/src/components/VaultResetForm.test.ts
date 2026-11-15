import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import VaultResetForm from "./VaultResetForm.vue";

describe("VaultResetForm", () => {
  // Removing only vault.json leaves a Touch ID marker that refuses the next
  // vault, so the start-over instructions name both files.
  it("names every file to remove when starting over without the passphrase", () => {
    const wrapper = mount(VaultResetForm, { props: { modelValue: "", loading: false, error: "" } });
    expect(wrapper.text()).toContain("vault.json");
    expect(wrapper.text()).toContain("vault-biometric-binding");
  });
});
