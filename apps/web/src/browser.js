let unsaved = false;
const timingEnabled = new URLSearchParams(location.search).has("timings");
export function monotonicNow() {
  return performance.now();
}
export function recordWork(kind, started) {
  if (timingEnabled) console.debug("NyatiDraw timing " + JSON.stringify({
    kind, ms: performance.now() - started,
  }));
}
export function backgroundTurn(delay) {
  return new Promise((resolve) => {
    setTimeout(() => {
      if (document.hidden || !window.requestIdleCallback) {
        resolve();
      } else {
        window.requestIdleCallback(resolve, { timeout: 250 });
      }
    }, delay);
  });
}
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
let workerAssets;
function recoveryAssets() {
  if (!workerAssets) workerAssets = (async () => {
    const response = await fetch(new URL("./worker/manifest.json", document.baseURI), { cache: "no-store" });
    if (!response.ok) throw new Error("저장 Worker 파일을 읽지 못했습니다.");
    const manifest = await response.json();
    if (manifest.protocol !== 1 || !/^recovery-[a-f0-9]+\.js$/.test(manifest.entry)
        || !/^storage-[a-f0-9]+\.js$/.test(manifest.storage)) {
      throw new Error("저장 Worker 버전이 맞지 않습니다. 새로고침해주세요.");
    }
    return manifest;
  })();
  return workerAssets;
}
export async function loadWorkspace() {
  const assets = await recoveryAssets();
  const storage = await import(new URL(`./worker/${assets.storage}`, document.baseURI).href);
  return storage.loadWorkspace();
}
let recoveryWorker;
let recoveryInFlight;
let recoveryId = 0;
let recoveryBroken;
function breakRecovery(error) {
  recoveryBroken = error;
  recoveryWorker?.terminate();
  recoveryInFlight?.reject(error);
  recoveryInFlight = null;
}
export async function saveInWorker(packet, download) {
  if (recoveryBroken) throw recoveryBroken;
  if (recoveryInFlight) throw new Error("저장 요청이 이미 진행 중입니다.");
  const assets = await recoveryAssets();
  if (!recoveryWorker) {
    recoveryWorker = new Worker(new URL(`./worker/${assets.entry}`, document.baseURI), {
      type: "module", name: "NyatiDraw recovery",
    });
    recoveryWorker.onerror = (event) => breakRecovery(new Error(event.message || "저장 Worker 실행 실패"));
    recoveryWorker.onmessageerror = () => breakRecovery(new Error("저장 Worker 응답 오류"));
    recoveryWorker.onmessage = ({ data }) => {
      const pending = recoveryInFlight;
      if (!pending || pending.id !== data.id) {
        breakRecovery(new Error("저장 Worker 응답 순서 오류"));
        return;
      }
      recoveryInFlight = null;
      if (data.error) pending.reject(new Error(data.error));
      else {
        if (timingEnabled && data.metrics) {
          const { encodeStart, encodeEnd, ...metrics } = data.metrics;
          metrics.mainFramesDuringEncode = pending.frames.filter(time => time >= encodeStart && time <= encodeEnd).length;
          console.debug("NyatiDraw worker " + JSON.stringify(metrics));
        }
        pending.resolve(data.bytes);
      }
    };
  }
  const id = ++recoveryId;
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => breakRecovery(new Error("저장 Worker 응답 시간이 초과되었습니다. 작업 파일을 내려받아주세요.")), 120000);
    let frame;
    const frames = [];
    const finish = () => { clearTimeout(timeout); if (frame) cancelAnimationFrame(frame); };
    recoveryInFlight = { id, frames,
      resolve: (value) => { finish(); resolve(value); },
      reject: (error) => { finish(); reject(error); },
    };
    if (timingEnabled) {
      const sampleFrame = () => {
        frames.push(performance.timeOrigin + performance.now());
        if (recoveryInFlight?.id === id && frames.length < 1024) frame = requestAnimationFrame(sampleFrame);
      };
      frame = requestAnimationFrame(sampleFrame);
    }
    try { recoveryWorker.postMessage({ id, packet, download, timings: timingEnabled }, [packet.buffer]); }
    catch (error) { breakRecovery(error); }
  });
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
export function mountCanvas() {
  const canvas = document.createElement("canvas");
  canvas.id = "drawing-canvas";
  canvas.tabIndex = 0;
  canvas.setAttribute("aria-label", "그리기 캔버스");
  const attach = () => {
    const slot = document.getElementById("drawing-canvas-slot");
    if (slot && canvas.parentElement !== slot) {
      cancelCanvasGesture?.();
      slot.appendChild(canvas);
    }
  };
  attach();
  new MutationObserver(attach).observe(document.body, { childList: true, subtree: true });
}

export function bindCanvas(canvas, callback) {
  if (timingEnabled) {
    const dispatch = callback;
    callback = (...args) => {
      const started = monotonicNow();
      try {
        return dispatch(...args);
      } finally {
        if (["begin", "end"].includes(args[0])) recordWork(args[0], started);
      }
    };
  }
  let active = null;
  let mode = null;
  let space = false;
  let pickerEvent = null;
  let pickerFrame = 0;
  let pickerTime = 0;
  const clearPicker = () => {
    cancelAnimationFrame(pickerFrame);
    pickerFrame = 0;
    pickerEvent = null;
  };
  const flushPicker = () => {
    pickerFrame = 0;
    if (active === null || mode !== "pick" || !pickerEvent) return;
    if (performance.now() - pickerTime < 33) {
      pickerFrame = requestAnimationFrame(flushPicker);
      return;
    }
    pickerTime = performance.now();
    const event = pickerEvent;
    pickerEvent = null;
    send("pickMove", event);
  };
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
      pickerEvent = event;
      if (!pickerFrame) pickerFrame = requestAnimationFrame(flushPicker);
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
    clearPicker();
    send(mode === "pan" ? "panEnd" : mode === "pick" ? "pickEnd" : "end", event);
    active = null;
    mode = null;
    canvas.releasePointerCapture(event.pointerId);
  });
  const cancel = () => {
    clearPicker();
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
