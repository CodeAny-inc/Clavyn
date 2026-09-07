import { beforeEach, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount } from "@vue/test-utils";
import AppSidebar from "./AppSidebar.vue";
import { useUiStore } from "../stores/ui";
import { useUpdateStore } from "../stores/update";

beforeEach(() => setActivePinia(createPinia()));

it("keeps vault and update status visible in the collapsed icon rail", async () => {
  const ui = useUiStore();
  const update = useUpdateStore();
  ui.sidebarCollapsed = true;
  update.info = {
    available: true,
    version: "0.2.0",
    current_version: "0.1.0",
    date: null,
    body: null,
  };

  const wrapper = mount(AppSidebar, {
    props: { activeView: "hosts", vaultUnlocked: true, tabCount: 2 },
  });

  expect(wrapper.get('[data-testid="compact-vault-status"]').classes()).toContain("bg-emerald-500");
  expect(wrapper.find('[data-testid="compact-update-status"]').exists()).toBe(true);
  expect(wrapper.find('button[aria-label="Settings, update available"]').exists()).toBe(true);

  await wrapper.setProps({ vaultUnlocked: false });
  expect(wrapper.get('[data-testid="compact-vault-status"]').classes()).toContain("bg-muted-foreground");
  expect(wrapper.find('button[aria-label="Vault, locked"]').exists()).toBe(true);
});
