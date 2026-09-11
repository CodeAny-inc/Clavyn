import { type ClassValue, clsx } from "clsx";

// Class conflicts are not resolved here. Tailwind decides a conflict by
// stylesheet order, not by the order of names in the attribute, so a caller
// that needs to override a component's base utility must mark it important
// (`!text-[11px]`) rather than relying on the merge order.
export function cn(...inputs: ClassValue[]) {
  return clsx(inputs);
}
