import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { getStartupFailure } from "./api";
import { applyAppearance, onSystemAppearanceChange } from "./lib/appearance";
import { storedAppearance } from "./stores/settings";
import "./styles.css";

// Applied before anything mounts, so a dark setup never flashes the light
// palette at startup, the recovery screen included.
applyAppearance(storedAppearance());
// Kept in step with a system theme change from here on, whichever screen
// mounts; the settings store applies changes made in the app itself.
onSystemAppearanceChange(() => applyAppearance(storedAppearance()));

// When the backend could not load its saved state, none of the app's commands
// are available, so the recovery screen is shown instead of the app. A failed
// query is not a startup failure: the app then boots and reports its own
// errors as usual.
const failure = await getStartupFailure().catch(() => null);
if (failure) {
  const { default: StartupRecovery } = await import("./components/StartupRecovery.vue");
  createApp(StartupRecovery, { failure }).mount("#app");
} else {
  const app = createApp(App);
  app.use(createPinia());
  app.mount("#app");
}
