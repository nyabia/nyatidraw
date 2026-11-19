const status = document.getElementById("startup-status");
try {
  const { default: initialize, browser_action, canvas_key, run } = await import("./pkg/editor.js");
  await initialize();
  document.addEventListener("keydown", event => {
    if (event.target.id !== "drawing-canvas" || event.isComposing || event.code === "Space") return;
    const modifiers = Number(event.ctrlKey) | Number(event.shiftKey) << 1 |
      Number(event.altKey) << 2 | Number(event.metaKey) << 3;
    const key = /^Key[A-Z]$/.test(event.code) ? event.code.slice(3).toLowerCase() : event.key.toLowerCase();
    if (canvas_key(key, modifiers, event.repeat)) {
      event.preventDefault();
      event.stopPropagation();
    }
  });
  document.body.addEventListener("click", event => {
    const button = event.target.closest("button[data-action]");
    if (!button || button.disabled) return;
    const [width, height] = (button.dataset.size || "0x0").split("x").map(Number);
    browser_action(button.dataset.action, width, height);
  });
  document.getElementById("project-dialog").addEventListener("cancel", event => {
    event.preventDefault();
    browser_action("close", 0, 0);
  });
  run();
} catch (error) {
  status.textContent = `편집기를 열지 못했습니다. ${error.message || error}`;
}
