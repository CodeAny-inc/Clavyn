// The app's light and dark palettes live in styles.css under :root and .dark;
// this decides which one is on.

export type Appearance = "system" | "light" | "dark";

export const APPEARANCES: readonly Appearance[] = ["system", "light", "dark"];

function systemQuery(): MediaQueryList | undefined {
  return typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : undefined;
}

export function prefersDark(appearance: Appearance): boolean {
  return appearance === "dark" || (appearance === "system" && !!systemQuery()?.matches);
}

/**
 * Put the document in the palette `appearance` asks for. `color-scheme` is
 * set alongside the class so native controls (selects, scrollbars, the date
 * of a file input) follow the same palette as everything drawn in CSS.
 */
export function applyAppearance(appearance: Appearance) {
  const dark = prefersDark(appearance);
  const root = document.documentElement;
  root.classList.toggle("dark", dark);
  root.style.colorScheme = dark ? "dark" : "light";
}

/** Re-apply whenever the system theme changes. Returns the unsubscribe. */
export function onSystemAppearanceChange(listener: () => void): () => void {
  const query = systemQuery();
  if (!query) return () => {};
  query.addEventListener("change", listener);
  return () => query.removeEventListener("change", listener);
}
