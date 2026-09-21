let unsaved = false;
window.addEventListener("beforeunload", (event) => {
  if (!unsaved) return;
  event.preventDefault();
  event.returnValue = "";
});
export function setUnsaved(value) {
  unsaved = value;
}

let releaseLock;
export async function claimWorkspace() {
  if (!navigator.locks)
    throw new Error(
      "안전한 작업 복구를 지원하는 최신 Chrome 또는 Edge를 이용해주세요.",
    );
  return new Promise((resolve, reject) => {
    navigator.locks
      .request("nyatidraw-web-workspace-v1", { ifAvailable: true }, (lock) => {
        if (!lock) {
          resolve(false);
          return;
        }
        resolve(true);
        return new Promise((done) => {
          releaseLock = done;
        });
      })
      .catch(reject);
  });
}
function database() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("nyatidraw-web-v1", 1);
    request.onupgradeneeded = () =>
      request.result.createObjectStore("workspace");
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
    request.onblocked = () =>
      reject(
        new Error("다른 탭에서 저장소를 사용 중입니다. 해당 탭을 닫아주세요."),
      );
  });
}
export async function loadWorkspace() {
  const db = await database();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction("workspace", "readonly");
      const request = tx.objectStore("workspace").get("current");
      request.onsuccess = () => resolve(request.result || null);
      request.onerror = () => reject(request.error);
    });
  } finally {
    db.close();
  }
}
export async function storeWorkspace(bytes) {
  const db = await database();
  try {
    await new Promise((resolve, reject) => {
      const tx = db.transaction("workspace", "readwrite");
      const store = tx.objectStore("workspace");
      const previous = store.get("current");
      previous.onsuccess = () => {
        const oldBytes = previous.result;
        if (oldBytes && String.fromCharCode(...oldBytes.slice(0, 8)) === "NYWEB001") {
          store.put(oldBytes, "legacy-before-ntdr");
        }
        store.put(bytes, "current");
      };
      tx.oncomplete = resolve;
      tx.onabort = () =>
        reject(tx.error || new Error("브라우저 저장이 취소되었습니다."));
      tx.onerror = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}
export function downloadBytes(bytes, filename, mime) {
  const url = URL.createObjectURL(new Blob([bytes], { type: mime }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60000);
}
export function pickFile() {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".ntdr,.png,.nyatidraw-web";
    input.hidden = true;
    document.body.append(input);
    input.oncancel = () => {
      input.remove();
      resolve(null);
    };
    input.onchange = async () => {
      const file = input.files[0];
      if (!file) {
        input.remove();
        resolve(null);
        return;
      }
      const limitMiB = file.name.toLowerCase().endsWith(".png") ? 72 : 128;
      if (file.size > limitMiB * 1024 * 1024) {
        input.remove();
        reject(new Error(`이 파일은 웹판의 ${limitMiB} MiB 한도를 넘습니다.`));
        return;
      }
      try {
        resolve([file.name, new Uint8Array(await file.arrayBuffer())]);
      } catch (error) {
        reject(error);
      } finally {
        input.remove();
      }
    };
    input.click();
  });
}
let modalOpen = false;
let cancelCanvasGesture;
let previousFocus;
let canvasTool = "stroke";
export function setCanvasTool(tool) {
  canvasTool = tool;
}
export function loadPreference(key) {
  const value = localStorage.getItem(`nyatidraw-web-ui-v1-${key}`);
  if (value !== null && value.length > 1024) throw new Error("저장된 화면 설정이 너무 큽니다.");
  return value;
}
export function storePreference(key, value) {
  if (value.length > 1024) throw new Error("화면 설정 크기 한도를 넘었습니다.");
  localStorage.setItem(`nyatidraw-web-ui-v1-${key}`, value);
}
export function setModalOpen(open) {
  if (open && !modalOpen) previousFocus = document.activeElement;
  modalOpen = open;
  if (open) {
    cancelCanvasGesture?.();
  } else {
    requestAnimationFrame(() => {
      if (!modalOpen && previousFocus?.isConnected) {
        previousFocus.focus({ preventScroll: true });
      }
    });
  }
}
export function focusModal() {
  const dialog = document.querySelector('.modal-shade [role="dialog"]');
  const target =
    dialog?.querySelector("[data-dialog-initial]") ||
    dialog?.querySelector("button, [href], input");
  (target || dialog)?.focus({ preventScroll: true });
}
function containDialogFocus(event) {
  const dialog = document.querySelector('.modal-shade [role="dialog"]');
  if (!dialog) return;
  const targets = [
    ...dialog.querySelectorAll(
      'button:not(:disabled), [href], input:not(:disabled), [tabindex="0"]',
    ),
  ];
  const first = targets[0];
  const last = targets[targets.length - 1];
  if (!first) {
    event.preventDefault();
    dialog.focus();
  } else if (
    event.shiftKey &&
    (document.activeElement === first ||
      !dialog.contains(document.activeElement))
  ) {
    event.preventDefault();
    last.focus();
  } else if (
    !event.shiftKey &&
    (document.activeElement === last ||
      !dialog.contains(document.activeElement))
  ) {
    event.preventDefault();
    first.focus();
  }
}
export function bindCanvas(canvas, callback) {
  let active = null;
  let mode = null;
  let space = false;
  const local = (event) => {
    const rect = canvas.getBoundingClientRect();
    return [event.clientX - rect.left, event.clientY - rect.top];
  };
  const send = (phase, event) => {
    const [x, y] = local(event);
    callback(
      phase,
      x,
      y,
      event.pointerType === "mouse" ? 1 : event.pressure,
      event.timeStamp,
    );
  };
  canvas.addEventListener("contextmenu", (event) => event.preventDefault());
  canvas.addEventListener("pointerdown", (event) => {
    if (
      modalOpen ||
      active !== null ||
      !event.isPrimary ||
      ![0, 1].includes(event.button)
    )
      return;
    event.preventDefault();
    document.activeElement?.blur();
    canvas.focus({ preventScroll: true });
    active = event.pointerId;
    mode =
      event.button === 1 || space || event.pointerType === "touch" || canvasTool === "pan"
        ? "pan"
        : event.altKey || canvasTool === "pick" ? "pick" : "stroke";
    canvas.setPointerCapture(active);
    send(mode === "pan" ? "panBegin" : mode === "pick" ? "pickBegin" : "begin", event);
  });
  canvas.addEventListener("pointermove", (event) => {
    if (modalOpen || event.pointerId !== active) return;
    event.preventDefault();
    if (mode === "pan") {
      send("panMove", event);
      return;
    }
    if (mode === "pick") {
      send("pickMove", event);
      return;
    }
    const samples = event.getCoalescedEvents?.() || [];
    if (samples.length > 256) {
      callback("overflow", 0, 0, 0, event.timeStamp);
      active = null;
      mode = null;
      return;
    }
    for (const sample of samples.length ? samples : [event])
      send("move", sample);
  });
  canvas.addEventListener("pointerup", (event) => {
    if (modalOpen || event.pointerId !== active) return;
    send(mode === "pan" ? "panEnd" : mode === "pick" ? "pickEnd" : "end", event);
    active = null;
    mode = null;
    canvas.releasePointerCapture(event.pointerId);
  });
  const cancel = () => {
    space = false;
    if (active === null) return;
    const captured = active;
    active = null;
    mode = null;
    callback("cancel", 0, 0, 0, 0);
    if (canvas.hasPointerCapture(captured))
      canvas.releasePointerCapture(captured);
  };
  cancelCanvasGesture = cancel;
  canvas.addEventListener("pointercancel", cancel);
  canvas.addEventListener("lostpointercapture", cancel);
  window.addEventListener("blur", () => {
    cancel();
    space = false;
  });
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) cancel();
  });
  canvas.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      if (modalOpen || active !== null) return;
      const [x, y] = local(event);
      callback("zoom", x, y, Math.max(-120, Math.min(120, event.deltaY)), 0);
    },
    { passive: false },
  );
  window.addEventListener("keydown", (event) => {
    if (modalOpen) {
      const key = event.key.toLowerCase();
      if (key === "escape") {
        event.preventDefault();
        event.stopPropagation();
        callback("dismissModal", 0, 0, 0, 0);
      } else if (key === "tab") {
        containDialogFocus(event);
      } else if (
        (event.ctrlKey || event.metaKey) &&
        ["s", "z", "y"].includes(key)
      ) {
        event.preventDefault();
      }
      return;
    }
    if (
      ["INPUT", "SELECT", "TEXTAREA"].includes(event.target.tagName) ||
      event.target.isContentEditable
    )
      return;
    if (event.code === "Space") {
      space = true;
      event.preventDefault();
      return;
    }
    const key = event.key.toLowerCase();
    if (key === "escape") {
      cancel();
      return;
    }
  });
  window.addEventListener("keyup", (event) => {
    if (event.code === "Space") space = false;
  });
  const resize = new ResizeObserver(() => callback("resize", 0, 0, 0, 0));
  resize.observe(canvas);
}
