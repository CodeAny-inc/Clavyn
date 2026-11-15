<script setup lang="ts">
import { nextTick, onUnmounted, ref, watch } from "vue";
import { useVaultStore } from "../stores/vault";
import { answerConfirmation, pendingConfirmation } from "../composables/useConfirm";
import Dialog from "./ui/Dialog.vue";
import Button from "./ui/Button.vue";

const vault = useVaultStore();
const cancelButton = ref<InstanceType<typeof Button> | null>(null);
const confirmButton = ref<InstanceType<typeof Button> | null>(null);

// A lock ends the session the question was asked in, so whatever is on screen
// is declined rather than left for whoever unlocks next.
watch(
  () => vault.unlocked,
  (unlocked) => {
    if (!unlocked) answerConfirmation(false);
  },
  { flush: "sync" },
);

// Focus starts on the safe answer for a destructive action, so an Enter or
// Space meant for something else cannot confirm it.
watch(pendingConfirmation, async (request) => {
  if (!request) return;
  await nextTick();
  const target = request.destructive ? cancelButton.value : confirmButton.value;
  (target?.$el as HTMLElement | undefined)?.focus();
});

// Escape is the shared dialog's: it takes the key at the document, wherever
// focus is, and only while this is the topmost modal. A handler of its own here
// would also answer when another modal is on top and the question is hidden.
onUnmounted(() => answerConfirmation(false));
</script>

<template>
  <Dialog
    :open="!!pendingConfirmation"
    :title="pendingConfirmation?.title"
    :description="pendingConfirmation?.message"
    role="alertdialog"
    width="420px"
    dismiss-on-overlay
    @close="answerConfirmation(false)"
  >
    <template #footer>
      <Button ref="cancelButton" variant="ghost" @click="answerConfirmation(false)">Cancel</Button>
      <Button
        ref="confirmButton"
        :variant="pendingConfirmation?.destructive ? 'destructive' : 'default'"
        data-testid="confirm-action"
        @click="answerConfirmation(true)"
      >
        {{ pendingConfirmation?.confirmLabel }}
      </Button>
    </template>
  </Dialog>
</template>
