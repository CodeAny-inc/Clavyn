<script setup lang="ts">
import { ref } from "vue";
import { AlertTriangle } from "lucide-vue-next";
import { restartApp, setAsideUnreadableFile, type StartupFailure } from "../api";
import Button from "./ui/Button.vue";

const props = defineProps<{ failure: StartupFailure }>();

const movedTo = ref<string | null>(null);
const error = ref<string | null>(null);
const busy = ref(false);

async function setAside() {
  busy.value = true;
  error.value = null;
  try {
    movedTo.value = await setAsideUnreadableFile();
  } catch (e) {
    error.value = String(e);
  } finally {
    busy.value = false;
  }
}

async function restart() {
  busy.value = true;
  try {
    await restartApp();
  } catch (e) {
    error.value = String(e);
    busy.value = false;
  }
}
</script>

<template>
  <div class="flex h-screen w-screen items-center justify-center overflow-y-auto bg-background p-6 text-foreground">
    <div class="w-full max-w-[520px] rounded-lg border border-border bg-card p-6" role="alert">
      <div class="mb-4 flex items-center gap-3">
        <AlertTriangle class="size-5 text-destructive" :stroke-width="1.75" />
        <h1 class="text-[15px] font-semibold">
          Clavyn could not load {{ props.failure.file_name ?? "its saved data" }}
        </h1>
      </div>
      <p class="mb-3 text-[13px] text-muted-foreground" data-testid="startup-reason">{{ props.failure.reason }}</p>

      <template v-if="props.failure.file">
        <p class="mb-3 text-[13px]">
          Clavyn does not start on a file it cannot read, and does not replace it on its own:
          starting empty would overwrite it. You can move it aside and start without it. The file
          is renamed, not deleted, so nothing in it is lost.
        </p>
        <p v-if="props.failure.file_name === 'vault.json'" class="mb-3 text-[13px]" data-testid="vault-note">
          vault.json holds your stored SSH keys, encrypted. Without it Clavyn starts with no
          stored keys until the file is repaired and put back.
        </p>
        <p class="mb-4 break-all font-mono text-[12px] text-muted-foreground">{{ props.failure.file }}</p>
      </template>

      <p v-if="movedTo" class="mb-4 text-[13px]" data-testid="moved-to">
        Moved to <span class="break-all font-mono text-[12px]">{{ movedTo }}</span>. Restart to continue.
      </p>
      <p v-if="error" class="mb-4 text-[13px] text-destructive">{{ error }}</p>

      <div class="flex flex-wrap gap-2">
        <Button v-if="props.failure.file && !movedTo" :disabled="busy" @click="setAside">
          Move aside
        </Button>
        <Button :variant="props.failure.file && !movedTo ? 'outline' : 'default'" :disabled="busy" @click="restart">
          {{ movedTo ? "Restart" : "Try again" }}
        </Button>
      </div>
    </div>
  </div>
</template>
