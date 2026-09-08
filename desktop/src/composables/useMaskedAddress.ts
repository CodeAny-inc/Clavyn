import { computed } from "vue";
import { useSettingsStore } from "../stores/settings";
import { maskAddress as maskAddressImpl } from "../lib/maskAddress";

/**
 * Reactive address masking bound to the user's privacy setting. Returns a
 * `maskAddress` function that obfuscates addresses when masking is enabled and
 * passes them through unchanged when the user opts to show full addresses.
 */
export function useMaskedAddress() {
  const settings = useSettingsStore();
  const enabled = computed(() => settings.maskAddresses);

  function maskAddress(address: string): string {
    if (!address || !enabled.value) return address;
    return maskAddressImpl(address);
  }

  return { maskAddress, enabled };
}
