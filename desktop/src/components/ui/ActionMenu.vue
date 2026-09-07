<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, useId, watch, type Component, type CSSProperties } from "vue";
import { MoreHorizontal } from "lucide-vue-next";

export interface MenuAction {
  id: string;
  label: string;
  icon?: Component;
  shortcut?: string;
  disabled?: boolean;
  danger?: boolean;
  separator?: boolean;
}
const props = withDefaults(defineProps<{ label: string; items: MenuAction[]; enabled?: boolean }>(), { enabled: true });
const emit = defineEmits<{ select: [id: string] }>();
const open = ref(false);
const trigger = ref<HTMLButtonElement | null>(null);
const menu = ref<HTMLElement | null>(null);
const position = ref<CSSProperties>({});
const id = useId();

function buttons() {
  return Array.from(menu.value?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []);
}
function close(restoreFocus = false) {
  open.value = false;
  document.removeEventListener("pointerdown", outside);
  window.removeEventListener("resize", dismiss);
  document.removeEventListener("scroll", scrolled, true);
  if (restoreFocus) trigger.value?.focus();
}
function dismiss() { close(); }
function scrolled(event: Event) { if (!(event.target instanceof Node) || !menu.value?.contains(event.target)) close(); }
function outside(event: PointerEvent) {
  if (!(event.target instanceof Node)) return;
  if (!menu.value?.contains(event.target) && !trigger.value?.contains(event.target)) close();
}
async function show(last = false) {
  if (props.enabled === false) return;
  open.value = true;
  await nextTick();
  if (!open.value || !trigger.value || !menu.value) return;
  const anchor = trigger.value.getBoundingClientRect();
  const bounds = menu.value.getBoundingClientRect();
  position.value = {
    left: `${Math.max(8, Math.min(anchor.right - bounds.width, innerWidth - bounds.width - 8))}px`,
    top: `${Math.max(8, Math.min(anchor.bottom + 6, innerHeight - bounds.height - 8))}px`,
  };
  const choices = buttons();
  choices[last ? choices.length - 1 : 0]?.focus();
  document.addEventListener("pointerdown", outside);
  window.addEventListener("resize", dismiss);
  document.addEventListener("scroll", scrolled, true);
}
function choose(item: MenuAction) {
  if (item.disabled) return;
  close(true);
  emit("select", item.id);
}
function keydown(event: KeyboardEvent) {
  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(true); return; }
  if (event.key === "Tab") { close(true); return; }
  const choices = buttons();
  const index = choices.findIndex(button => button === document.activeElement);
  let next: number;
  if (event.key === "ArrowDown") next = (index + 1) % choices.length;
  else if (event.key === "ArrowUp") next = (index - 1 + choices.length) % choices.length;
  else if (event.key === "Home") next = 0;
  else if (event.key === "End") next = choices.length - 1;
  else return;
  event.preventDefault();
  event.stopPropagation();
  choices[next]?.focus();
}
watch(() => props.enabled, enabled => { if (enabled === false) close(); });
onBeforeUnmount(() => close());
</script>

<template>
  <button ref="trigger" class="action-menu-trigger" :aria-label="label" :title="label"
    aria-haspopup="menu" :aria-expanded="open" :aria-controls="open ? id : undefined"
    @click.stop="open ? close(true) : show()"
    @keydown.down.prevent.stop="show()" @keydown.up.prevent.stop="show(true)">
    <MoreHorizontal class="size-4" :stroke-width="1.75" />
  </button>
  <Teleport to="body">
    <div v-if="open" :id="id" ref="menu" class="action-menu" :style="position" role="menu"
      :aria-label="label" @keydown="keydown" @click.stop>
      <template v-for="item in items" :key="item.id">
        <div v-if="item.separator" class="my-1 border-t border-border" role="separator" />
        <button role="menuitem" class="action-menu-item" :class="{ 'text-destructive': item.danger }"
          :disabled="item.disabled" @click="choose(item)">
          <component :is="item.icon" v-if="item.icon" class="size-4 shrink-0" :stroke-width="1.75" />
          <span class="min-w-0 flex-1 truncate">{{ item.label }}</span>
          <span v-if="item.shortcut" class="ml-3 text-[11px] text-muted-foreground">{{ item.shortcut }}</span>
        </button>
      </template>
    </div>
  </Teleport>
</template>

<style scoped>
.action-menu-trigger { @apply inline-flex size-8 shrink-0 items-center justify-center rounded-md hover:bg-white/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring; }
.action-menu { @apply fixed z-[120] max-h-[calc(100dvh-16px)] w-64 max-w-[calc(100vw-16px)] overflow-y-auto rounded-xl border border-border bg-popover p-1.5 text-popover-foreground shadow-xl; }
.action-menu-item { @apply flex min-h-9 w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-[13px] hover:bg-muted focus:bg-muted focus:outline-none disabled:opacity-40; }
</style>
