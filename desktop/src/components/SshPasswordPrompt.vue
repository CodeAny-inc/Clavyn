<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, useId, watch } from "vue";
import { useFocusIntent } from "../composables/useFocusIntent";

const props = defineProps<{ active: boolean }>();
const emit = defineEmits<{ activate: []; finished: [] }>();
const { capture: captureFocusIntent, blocked: terminalFocusBlocked } = useFocusIntent();
const pending = ref(false);
const endpoint = ref("");
const password = ref("");
const input = ref<HTMLInputElement | null>(null);
const headingId = useId();
let complete: ((value: string | null) => void) | undefined;

function focus() {
  if (props.active && pending.value && !terminalFocusBlocked()) input.value?.focus();
}
function finish(value: string | null, restoreFocus = true) {
  const resolve = complete;
  complete = undefined;
  // Clear both reactive state and the live input before dispatch/cancellation.
  password.value = "";
  if (input.value) input.value.value = "";
  pending.value = false;
  endpoint.value = "";
  // Restore on the user's submit/cancel, not on a later network completion.
  if (resolve && restoreFocus) emit("finished");
  resolve?.(value);
}
function cancel() { finish(null); }
function request(address: string, mayAutofocus: () => boolean = () => true): Promise<string | null> {
  finish(null, false);
  endpoint.value = address;
  pending.value = true;
  const current = captureFocusIntent();
  const result = new Promise<string | null>(resolve => { complete = resolve; });
  void nextTick(() => { if (current() && mayAutofocus()) focus(); });
  return result;
}
// Terminal panes intentionally stay mounted across tabs/views. Once this pane
// loses ownership, discard any unsubmitted credential instead of retaining it
// in a hidden component. A future reconnect requests a fresh password.
watch(() => props.active, active => {
  if (!active && pending.value) finish(null, false);
}, { flush: "sync" });
onBeforeUnmount(() => finish(null, false));
defineExpose({ request, cancel, focus, pending });
</script>

<template>
  <!-- Pane-local, not a global modal: background connections cannot steal focus. -->
  <div v-if="pending" class="absolute inset-x-2 top-12 z-50 rounded-xl border border-border bg-background p-4 text-foreground shadow-xl"
    role="region" :aria-labelledby="headingId" @click.stop @focusin="emit('activate')"
    @keydown.stop @keydown.esc.prevent="cancel">
    <form @submit.prevent="finish(password)" autocomplete="off">
      <h2 :id="headingId" class="text-sm font-semibold">SSH password</h2>
      <p class="my-2 break-all text-xs text-muted-foreground">{{ endpoint }}</p>
      <input ref="input" v-model="password" type="password" aria-label="SSH password"
        autocomplete="off" autocapitalize="off" :spellcheck="false"
        class="h-9 w-full rounded-md border border-border bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" />
      <p class="mt-2 text-xs text-muted-foreground">Used for this connection only. Not saved.</p>
      <div class="mt-3 flex justify-end gap-2">
        <button type="button" class="rounded-md px-3 py-2 text-xs hover:bg-muted" @click="cancel">Cancel connection</button>
        <button type="submit" class="rounded-md bg-primary px-3 py-2 text-xs text-primary-foreground">Connect with password</button>
      </div>
    </form>
  </div>
</template>
