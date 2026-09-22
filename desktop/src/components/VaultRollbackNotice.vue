<script setup lang="ts">
import { AlertTriangle } from "lucide-vue-next";
import Button from "./ui/Button.vue";

defineProps<{ message: string; disabled?: boolean }>();
defineEmits<{ open: [] }>();
</script>

<template>
  <div role="alert" class="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-[12px]" data-testid="vault-rollback">
    <div class="flex items-start gap-2">
      <AlertTriangle class="size-3.5 shrink-0 mt-0.5 text-destructive" :stroke-width="1.75" />
      <p>{{ message }}</p>
    </div>
    <Button class="mt-3" size="sm" variant="outline" :disabled="disabled" @click="$emit('open')">
      Open this older copy
    </Button>
  </div>
</template>
