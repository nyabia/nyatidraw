const slot = document.getElementById("drawing-canvas-slot");
const status = document.getElementById("startup-status");
const dialog = document.getElementById("project-dialog");
const bar = document.getElementById("integration");
const noticeBox = document.getElementById("notice");
let previousBounds = "";
let previousModal = "";

export function position_canvas(x, y, width, height) {
  const css = `left:${x}px;top:${y}px;width:${Math.max(0, width)}px;height:${Math.max(0, height)}px`;
  if (previousBounds !== css) {
    slot.style.cssText = css;
    previousBounds = css;
  }
}

export function show_canvas(visible) {
  if (slot.hidden === !visible) return;
  if (!visible) slot.querySelector("canvas")?.dispatchEvent(new Event("nyati-hide-canvas"));
  slot.hidden = !visible;
}

export function canvas_dock_drag(active) {
  slot.toggleAttribute("data-dock-drag", active);
}

export function panel_visible(key) {
  const dataKey = "nyati" + key.charAt(0).toUpperCase() + key.slice(1);
  return document.documentElement.dataset[dataKey] !== "false";
}

export function startup_error(text) {
  status.hidden = false;
  status.querySelector("span").textContent = text;
}

export function display_recovery(description, available) {
  slot.setAttribute("aria-description", description);
  const recovery = document.getElementById("recovery-status");
  recovery.hidden = !description;
  recovery.textContent = description;
  status.querySelector("button").hidden = !available;
}

export function display_picker(x, y, image, color, visible) {
  const picker = document.getElementById("picker");
  picker.hidden = !visible;
  if (!visible) return;
  picker.style.left = `clamp(6px, ${x - 55}px, calc(100vw - 116px))`;
  picker.style.top = `clamp(6px, ${y >= 150 ? y - 144 : y + 26}px, calc(100vh - 140px))`;
  const preview = picker.querySelector("img");
  preview.hidden = !image;
  if (image) preview.src = image;
  picker.querySelector("span").textContent = color || "투명";
}

export function display_status(text, notice, modal, ready, busy, hosted) {
  status.hidden = ready;
  if (!ready) status.querySelector("span").textContent = text;
  bar.hidden = !hosted;
  document.body.classList.toggle("hosted", hosted);
  for (const control of bar.querySelectorAll("button")) control.disabled = !ready || busy;
  noticeBox.hidden = !notice;
  if (notice) noticeBox.querySelector("span").textContent = notice;
  if (previousModal === modal) return;
  previousModal = modal;
  if (!modal) {
    if (dialog.open) dialog.close();
    return;
  }
  const titles = { new: "새 그림", replace: "현재 그림을 바꿀까요?", cancel: "편집을 취소할까요?", help: "NyatiDraw" };
  dialog.querySelector("h2").textContent = titles[modal];
  for (const group of dialog.querySelectorAll("[data-group]")) group.hidden = group.dataset.group !== modal;
  if (!dialog.open) dialog.showModal();
  dialog.querySelector('[data-action="close"]').focus();
}
