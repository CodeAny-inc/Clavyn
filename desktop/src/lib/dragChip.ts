// A compact drag ghost for session-tab and pane-grip drags: a small chip
// showing the dragged session's title, in place of the browser's default
// full-element snapshot. Appended to <body> so clipped ancestors cannot hide
// it; removed when the drag source's dragend fires.
export function setDragImageChip(event: DragEvent, label: string) {
  const dt = event.dataTransfer;
  const source = event.currentTarget;
  if (!dt || !(source instanceof HTMLElement)) return;
  const chip = document.createElement("div");
  chip.className = "drag-chip";
  chip.textContent = label;
  document.body.appendChild(chip);
  try {
    dt.setDragImage(chip, Math.min(chip.offsetWidth / 2, 80), 14);
  } catch {
    // Webviews that ignore custom drag images fall back to the element snapshot.
  }
  source.addEventListener("dragend", () => chip.remove(), { once: true });
}
