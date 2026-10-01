import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { getStartupFailure } from "./api";
import "./styles.css";

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
