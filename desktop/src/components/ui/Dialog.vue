<script setup lang="ts">
import { ref, useId } from "vue";
import { useModalFocus } from "../../composables/useModalFocus";
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

useModalFocus(panel, () => props.open, () => emit("close"));

function onOverlayClose() {
  if (props.dismissOnOverlay) emit("close");
}

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
