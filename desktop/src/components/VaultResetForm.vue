<script setup lang="ts">
import Button from "./ui/Button.vue";
import Input from "./ui/Input.vue";
import FormGroup from "./ui/FormGroup.vue";
import Label from "./ui/Label.vue";
import { AlertTriangle, Loader2, Trash2 } from "lucide-vue-next";

defineProps<{
  modelValue: string;
  loading: boolean;
  error: string;
}>();

defineEmits<{
  "update:modelValue": [value: string];
  confirm: [];
  cancel: [];
}>();
</script>

<template>
  <div class="flex flex-col gap-3">
    <div class="flex items-start gap-2.5 rounded-md border border-destructive/30 bg-destructive/5 p-3">
      <AlertTriangle class="size-4 text-destructive shrink-0 mt-0.5" :stroke-width="1.75" />
      <div class="text-[12px] text-muted-foreground">
        Resetting the vault <strong class="text-destructive">permanently deletes all stored SSH keys and credentials</strong>.
        This cannot be undone. Enter your master passphrase to confirm.
      </div>
    </div>
    <FormGroup>
      <Label for="reset-pass">Master passphrase</Label>
      <Input
        id="reset-pass"
        :model-value="modelValue"
        type="password"
        placeholder="Enter master passphrase to confirm"
        :disabled="loading"
        @update:model-value="$emit('update:modelValue', String($event))"
        @keydown.enter="$emit('confirm')"
      />
    </FormGroup>
    <p v-if="error" class="text-[12px] text-destructive">{{ error }}</p>
    <div class="flex items-center gap-2">
      <Button
        variant="destructive"
        size="sm"
        :disabled="loading || !modelValue"
        @click="$emit('confirm')"
      >
        <Loader2
          v-if="loading"
          class="size-3.5 mr-1 animate-spin"
          :stroke-width="1.75"
        />
        <Trash2 v-else class="size-3.5" :stroke-width="1.75" />
        {{ loading ? "Resetting..." : "Reset Vault" }}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        :disabled="loading"
        @click="$emit('cancel')"
      >
        Cancel
      </Button>
    </div>
  </div>
</template>
