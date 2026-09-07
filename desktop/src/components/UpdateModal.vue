<script setup lang="ts">
import { computed } from "vue";
import { useUpdateStore } from "../stores/update";
import Button from "./ui/Button.vue";
import {
  ArrowUpCircle,
  Download,
  X,
  Loader2,
  RefreshCw,
  ExternalLink,
  CheckCircle2,
  Sparkles,
} from "lucide-vue-next";

const update = useUpdateStore();

const progressPercent = computed(() => Math.round(update.progress));

function openReleases() {
  import("@tauri-apps/plugin-shell")
    .then(({ open }) => {
      open("https://github.com/CodeAny-inc/Clavyn/releases");
    })
    .catch(() => {
      window.open(
        "https://github.com/CodeAny-inc/Clavyn/releases",
        "_blank",
      );
    });
}
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition-all duration-220 ease-grok"
      enter-from-class="opacity-0 scale-95"
      leave-active-class="transition-all duration-140 ease-grok"
      leave-to-class="opacity-0 scale-95"
    >
      <div
        v-if="update.showModal && update.available"
        class="fixed inset-0 z-[110] flex items-center justify-center bg-black/50"
        @click.self="update.dismissForNow()"
      >
        <div class="w-[440px] max-w-[92vw] rounded-xl border border-border bg-card shadow-dialog overflow-hidden">
          <!-- Header with gradient -->
          <div class="relative bg-gradient-to-br from-primary/15 via-primary/5 to-transparent px-5 pt-5 pb-4">
            <button
              class="absolute top-3 right-3 flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted transition-colors duration-100"
              aria-label="Close"
              @click="update.dismissForNow()"
            >
              <X class="size-4" :stroke-width="1.75" />
            </button>
            <div class="flex items-center gap-3">
              <div class="flex h-11 w-11 items-center justify-center rounded-xl bg-primary/15 shrink-0">
                <Sparkles class="size-5 text-primary" :stroke-width="1.75" />
              </div>
              <div>
                <div class="text-[15px] font-semibold">Update Available</div>
                <div class="text-[12px] text-muted-foreground mt-0.5">
                  A new version of Clavyn is ready
                </div>
              </div>
            </div>
          </div>

          <!-- Body -->
          <div class="px-5 py-4">
            <!-- Version comparison -->
            <div class="flex items-center gap-3 rounded-lg border border-border bg-muted/30 p-3 mb-4">
              <div class="flex-1 text-center">
                <div class="text-[10px] uppercase tracking-wide text-muted-foreground mb-1">Current</div>
                <div class="text-[14px] font-mono font-medium text-muted-foreground">
                  v{{ update.currentVersion }}
                </div>
              </div>
              <ArrowUpCircle class="size-5 text-primary shrink-0" :stroke-width="1.75" />
              <div class="flex-1 text-center">
                <div class="text-[10px] uppercase tracking-wide text-muted-foreground mb-1">New</div>
                <div class="text-[14px] font-mono font-medium text-primary">
                  v{{ update.version }}
                </div>
              </div>
            </div>

            <!-- Release notes -->
            <div v-if="update.body" class="mb-4">
              <div class="text-[11px] font-medium text-muted-foreground uppercase tracking-wide mb-1.5">
                Release Notes
              </div>
              <div class="text-[12px] text-muted-foreground whitespace-pre-wrap rounded-md bg-muted/40 p-3 max-h-[160px] overflow-y-auto leading-relaxed">
                {{ update.body }}
              </div>
            </div>

            <!-- Download progress -->
            <div v-if="update.installing" class="mb-2">
              <div class="flex items-center justify-between mb-2">
                <div class="flex items-center gap-2 text-[12px] text-muted-foreground">
                  <Loader2 class="size-3.5 animate-spin" :stroke-width="1.75" />
                  <span v-if="update.extracting">Extracting and installing...</span>
                  <span v-else>Downloading... {{ progressPercent }}%</span>
                </div>
                <span class="text-[11px] text-muted-foreground tabular-nums">{{ progressPercent }}%</span>
              </div>
              <div class="h-1.5 w-full rounded-full bg-muted overflow-hidden">
                <div
                  class="h-full bg-primary transition-all duration-200 ease-grok"
                  :style="{ width: update.extracting ? '100%' : `${update.progress}%` }"
                />
              </div>
              <div class="text-[11px] text-muted-foreground mt-2 flex items-center gap-1.5">
                <CheckCircle2 class="size-3 text-green-500" :stroke-width="1.75" />
                The app will restart automatically after installation.
              </div>
            </div>

            <!-- Error -->
            <div
              v-if="update.error && !update.installing"
              class="flex items-center gap-2 rounded-md bg-destructive/10 p-2.5 mb-3"
            >
              <X class="size-3.5 text-destructive shrink-0" :stroke-width="1.75" />
              <span class="text-[12px] text-destructive">{{ update.error }}</span>
            </div>
          </div>

          <!-- Footer -->
          <div class="flex flex-wrap items-center gap-2 px-4 sm:px-5 py-3.5 border-t border-border bg-muted/20">
            <button
              class="inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-[12px] text-muted-foreground hover:bg-muted transition-colors duration-100"
              @click="openReleases"
            >
              <ExternalLink class="size-3" :stroke-width="1.75" />
              <span class="hidden sm:inline">Release Notes</span>
            </button>
            <div class="flex-1"></div>
            <template v-if="!update.installing">
              <Button variant="ghost" size="sm" class="hidden sm:inline-flex" @click="update.skipVersion()">
                Skip this version
              </Button>
              <Button variant="ghost" size="sm" @click="update.dismissForNow()">
                Later
              </Button>
              <Button size="sm" @click="update.install()">
                <Download class="size-3.5 mr-1" :stroke-width="1.75" />
                Update Now
              </Button>
            </template>
            <template v-else>
              <div class="flex items-center gap-1.5 text-[12px] text-muted-foreground">
                <Loader2 class="size-3.5 animate-spin" :stroke-width="1.75" />
                Installing...
              </div>
            </template>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
