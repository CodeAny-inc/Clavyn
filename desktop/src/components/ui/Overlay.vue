<script setup lang="ts">
import { cn } from "../../lib/cn";

withDefaults(
  defineProps<{
    class?: string;
  }>(),
  {},
);
</script>

<template>
  <div
    data-terminal-focus-blocker
    :class="cn(
      'fixed inset-0 z-50 bg-black/50',
      'animate-fade-in',
      $props.class,
    )"
    @click="$emit('close')"
  >
    <slot />
  </div>
</template>
