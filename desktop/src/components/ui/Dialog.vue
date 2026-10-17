<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, useId, watch } from "vue";
import { cn } from "../../lib/cn";
import Overlay from "./Overlay.vue";

const props = withDefaults(
  defineProps<{
    open: boolean;
    title?: string;
    description?: string;
    width?: string;
    class?: string;
    role?: "dialog" | "alertdialog";
    /** Close on a click outside the panel. Off by default: in a form, one
     * stray click there would throw away everything typed. */
    dismissOnOverlay?: boolean;
  }>(),
  {
    width: "480px",
    role: "dialog",
    dismissOnOverlay: false,
  },
);

const emit = defineEmits<{ close: [] }>();
const titleId = useId();
const descriptionId = useId();
const panel = ref<HTMLElement | null>(null);

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), ' +
  'select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusables(): HTMLElement[] {
  return Array.from(panel.value?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []).filter(
    (el) => !el.closest("[inert]") && el.getClientRects().length > 0,
  );
}

// Focus returns to whatever opened the dialog once it closes, so keyboard
// users are not dropped back at the top of the page.
let opener: HTMLElement | null = null;
watch(
  () => props.open,
  (open) => {
    if (open) opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    else restoreFocus();
  },
  { immediate: true },
);

function restoreFocus() {
  const target = opener;
  opener = null;
  const active = document.activeElement;
  // Only when focus is still in the dialog or nowhere: never pull it back from
  // somewhere the user has already moved to.
  const unclaimed = !active || active === document.body || !!panel.value?.contains(active);
  if (target?.isConnected && unclaimed) target.focus();
}

// The first field gets focus when the dialog appears, unless something inside
// it has already taken focus (a confirmation focuses its own safe button).
watch(panel, async (element) => {
  if (!element) return;
  await nextTick();
  if (element.contains(document.activeElement)) return;
  const fields = focusables();
  const first = fields.find((el) => el.matches("input, select, textarea")) ?? fields[0];
  (first ?? element).focus();
});

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    emit("close");
    return;
  }
  // Tab cycles within the dialog rather than reaching the page behind it.
  if (event.key !== "Tab") return;
  const fields = focusables();
  if (!fields.length) return;
  const first = fields[0];
  const last = fields[fields.length - 1];
  const active = document.activeElement;
  if (event.shiftKey && (active === first || !panel.value?.contains(active))) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && (active === last || !panel.value?.contains(active))) {
    event.preventDefault();
    first.focus();
  }
}

function onOverlayClose() {
  if (props.dismissOnOverlay) emit("close");
}

onBeforeUnmount(restoreFocus);
</script>

<template>
  <Teleport to="body">
    <Overlay v-if="props.open" @close="onOverlayClose">
      <div
        ref="panel"
        tabindex="-1"
        :class="cn(
          'fixed left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2',
          'z-50 w-full max-w-[92vw] max-h-[90vh]',
          'rounded-xl border border-border bg-popover text-popover-foreground shadow-dialog',
          'animate-scale-in',
          'flex flex-col',
          'mx-2',
          props.class,
        )"
        :style="{ maxWidth: props.width }"
        :role="props.role"
        aria-modal="true"
        :aria-labelledby="props.title && !$slots.header ? titleId : undefined"
        :aria-describedby="props.description && !$slots.header ? descriptionId : undefined"
        @click.stop
        @keydown="onKeydown"
      >
        <div v-if="props.title || $slots.header" class="px-4 sm:px-5 pt-4 sm:pt-5 pb-3">
          <slot name="header">
            <h2 :id="titleId" class="text-[15px] font-semibold leading-tight">{{ props.title }}</h2>
            <p v-if="props.description" :id="descriptionId" class="mt-1 text-[13px] text-muted-foreground">
              {{ props.description }}
            </p>
          </slot>
        </div>
        <div v-if="$slots.default" class="px-4 sm:px-5 py-3 overflow-y-auto flex-1">
          <slot />
        </div>
        <div
          v-if="$slots.footer"
          class="flex flex-wrap items-center justify-end gap-2 border-t border-border px-4 sm:px-5 py-3"
        >
          <slot name="footer" />
        </div>
      </div>
    </Overlay>
  </Teleport>
</template>
