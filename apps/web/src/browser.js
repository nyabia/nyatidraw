let unsaved = false;
const query = new URLSearchParams(location.search);
const embeddedMode = query.get("nyatiMode");
const hosted = embeddedMode === "embed" || embeddedMode === "popup";
const framed = window.parent !== window;
let integration;
let embedUi = {};
function integrationTarget() {
  if (embeddedMode === "embed" && framed) return window.parent;
  if (embeddedMode === "popup" && !framed) return window.opener;
  return null;
}
function integrationOrigin() {
  const value = query.get("hostOrigin");
  if (!value) return null;
  try {
    const parsed = new URL(value);
    if (parsed.origin !== value || parsed.username || parsed.password) return null;
    if (parsed.protocol === "https:" ||
        (parsed.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname))) return value;
  } catch {}
  return null;
}
function integrationSession() {
  const value = query.get("session");
  return value && /^[a-f0-9]{32,128}$/i.test(value) ? value : null;
}
function envelope(type, extra = {}) {
  return { channel: "nyatidraw", version: 1, session: integration.session, type, ...extra };
}
function sendIntegration(type, extra = {}, transfer = []) {
  integration.target.postMessage(envelope(type, extra), integration.origin, transfer);
}
const uiKeys = ["toolbar", "tools", "brush", "colors", "layers", "history", "navigator"];
function validateIntegrationConfig(config) {
  if (!config || typeof config !== "object" || Array.isArray(config)) throw new Error("초기 설정이 없습니다.");
  if (Object.keys(config).some(key => !["canvas", "tool", "size", "color", "opacity", "ui"].includes(key))) throw new Error("알 수 없는 초기 설정입니다.");
  const { canvas, tool = "pen", size = 10, color = "#000000", opacity = 1, ui = {} } = config;
  if (canvas && Object.keys(canvas).some(key => !["width", "height"].includes(key))) throw new Error("알 수 없는 캔버스 설정입니다.");
  if (!canvas || !Number.isInteger(canvas.width) || !Number.isInteger(canvas.height) ||
      canvas.width < 1 || canvas.height < 1 || canvas.width > 4096 || canvas.height > 4096 ||
      canvas.width * canvas.height > 16777216) throw new Error("캔버스 크기는 1~4096 픽셀 범위여야 합니다.");
  if (!["pen", "pencil2h", "pencil2b", "brush", "eraser"].includes(tool)) throw new Error("도구 설정이 올바르지 않습니다.");
  if (typeof size !== "number" || !Number.isFinite(size) || size < 0.1 || size > 200) throw new Error("브러시 크기가 올바르지 않습니다.");
  if (typeof color !== "string" || !/^#[a-f0-9]{6}$/i.test(color)) throw new Error("색상 설정이 올바르지 않습니다.");
  if (typeof opacity !== "number" || !Number.isFinite(opacity) || opacity < 0.01 || opacity > 1) throw new Error("불투명도 설정이 올바르지 않습니다.");
  if (!ui || typeof ui !== "object" || Array.isArray(ui) ||
      Object.keys(ui).some(key => !uiKeys.includes(key) || typeof ui[key] !== "boolean")) throw new Error("화면 설정이 올바르지 않습니다.");
  if (embeddedMode === "embed") {
    embedUi = ui;
    document.documentElement.classList.add("nyati-embed");
    for (const key of uiKeys) document.documentElement.dataset[`nyati${key[0].toUpperCase()}${key.slice(1)}`] = String(ui[key] !== false);
  }
  return [String(canvas.width), String(canvas.height), tool, String(Math.round(size * 10) / 10), color, String(opacity)];
}
export function isHosted() { return hosted; }
export function embedPrefersHistory() { return embeddedMode === "embed" && embedUi.layers === false && embedUi.history !== false; }
export function canStartStandalone() { return !query.has("nyatiMode") && !framed; }
export function beginIntegration() {
  return new Promise((resolve, reject) => {
    const target = integrationTarget();
    const origin = integrationOrigin();
    const session = integrationSession();
    if (!hosted || !target || !origin || !session) {
      reject(new Error("연결 주소가 올바르지 않습니다. 호스트에서 편집기를 다시 열어주세요."));
      return;
    }
    integration = { target, origin, session, pending: null };
    const timeout = setTimeout(() => {
      window.removeEventListener("message", onInit);
      reject(new Error("호스트 연결 시간이 초과되었습니다. 호스트 페이지에서 다시 열어주세요."));
    }, 15000);
    function onInit(event) {
      if (event.source !== target || event.origin !== origin) return;
      const data = event.data;
      if (!data || data.channel !== "nyatidraw" || data.version !== 1 || data.session !== session || data.type !== "init") return;
      clearTimeout(timeout);
      window.removeEventListener("message", onInit);
      try {
        const config = validateIntegrationConfig(data.config);
        window.addEventListener("message", onIntegrationMessage);
        resolve(config);
      } catch (error) {
        sendIntegration("error", { message: String(error.message || error) });
        reject(error);
      }
    }
    window.addEventListener("message", onInit);
    sendIntegration("ready");
  });
}
function onIntegrationMessage(event) {
  if (!integration || event.source !== integration.target || event.origin !== integration.origin) return;
  const data = event.data;
  if (!data || data.channel !== "nyatidraw" || data.version !== 1 || data.session !== integration.session) return;
  const pending = integration.pending;
  if (!pending || data.requestId !== pending.requestId) return;
  if (data.type === "ack") pending.finish(null);
  else if (data.type === "error" && typeof data.message === "string") pending.finish(new Error(data.message.slice(0, 500)));
}
export function integrationInitialized() { sendIntegration("initialized"); }
export function integrationError(message) {
  if (integration) sendIntegration("error", { message: String(message).slice(0, 500) });
}
export function integrationSend(type, bytes, width, height) {
  return new Promise((resolve, reject) => {
    if (!integration || integration.pending || !["complete", "cancel"].includes(type)) {
      reject(new Error("호스트 연결이 준비되지 않았습니다.")); return;
    }
    const requestId = crypto.randomUUID();
    const timeout = setTimeout(() => finish(new Error("호스트 응답 시간이 초과되었습니다. 다시 시도하거나 그림을 내려받아주세요.")), 15000);
    function finish(error) {
      clearTimeout(timeout);
      integration.pending = null;
      if (error) reject(error);
      else {
        setUnsaved(false);
        sendIntegration("closed", { requestId });
        if (embeddedMode === "popup") window.close();
        resolve(true);
      }
    }
    integration.pending = { requestId, finish };
    try {
      if (type === "complete") sendIntegration(type, { requestId, mime: "image/png", width, height, bytes: bytes.buffer }, [bytes.buffer]);
      else sendIntegration(type, { requestId });
    } catch (error) { finish(error); }
  });
}
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
  if (!canStartStandalone()) throw new Error("연결 세션은 독립 작업 공간을 사용하지 않습니다.");
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
  if (!canStartStandalone()) throw new Error("연결 세션은 이전 그림을 읽지 않습니다.");
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
  if (!canStartStandalone()) throw new Error("연결 세션은 브라우저 복구 공간에 저장하지 않습니다.");
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
  if (hosted || framed) return null;
  const value = localStorage.getItem(`nyatidraw-web-ui-v1-${key}`);
  if (value !== null && value.length > 1024) throw new Error("저장된 화면 설정이 너무 큽니다.");
  return value;
}
export function storePreference(key, value) {
  if (hosted || framed) return;
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
  let activePointerType = null;
  let lastPenActivity = -Infinity;
  const penTouchGuardMs = 700;
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
      event.pointerType === "pen",
    );
  };
  const releaseTouchPan = () => {
    const captured = active;
    active = null;
    activePointerType = null;
    mode = null;
    callback("panEnd", 0, 0, 0, 0, false);
    if (canvas.hasPointerCapture(captured)) canvas.releasePointerCapture(captured);
  };
  canvas.addEventListener("contextmenu", (event) => event.preventDefault());
  canvas.addEventListener("pointerdown", (event) => {
    if (event.pointerType === "pen") {
      lastPenActivity = performance.now();
      if (activePointerType === "touch") releaseTouchPan();
    }
    if (event.pointerType === "touch" &&
        performance.now() - lastPenActivity < penTouchGuardMs) {
      event.preventDefault();
      return;
    }
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
    activePointerType = event.pointerType;
    mode =
      event.button === 1 || space || event.pointerType === "touch" || canvasTool === "pan"
        ? "pan"
        : event.altKey || canvasTool === "pick" ? "pick" : "stroke";
    canvas.setPointerCapture(active);
    send(mode === "pan" ? "panBegin" : mode === "pick" ? "pickBegin" : "begin", event);
  });
  canvas.addEventListener("pointermove", (event) => {
    if (event.pointerType === "pen") lastPenActivity = performance.now();
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
      callback("overflow", 0, 0, 0, event.timeStamp, false);
      active = null;
      activePointerType = null;
      mode = null;
      return;
    }
    for (const sample of samples.length ? samples : [event])
      send("move", sample);
  });
  canvas.addEventListener("pointerup", (event) => {
    if (event.pointerType === "pen") lastPenActivity = performance.now();
    if (modalOpen || event.pointerId !== active) return;
    clearPicker();
    send(mode === "pan" ? "panEnd" : mode === "pick" ? "pickEnd" : "end", event);
    active = null;
    activePointerType = null;
    mode = null;
    canvas.releasePointerCapture(event.pointerId);
  });
  const cancel = () => {
    clearPicker();
    space = false;
    if (active === null) return;
    const captured = active;
    active = null;
    activePointerType = null;
    mode = null;
    callback("cancel", 0, 0, 0, 0, false);
    if (canvas.hasPointerCapture(captured))
      canvas.releasePointerCapture(captured);
  };
  cancelCanvasGesture = cancel;
  const cancelActivePointer = (event) => {
    if (event.pointerId !== active) return;
    if (activePointerType === "touch") releaseTouchPan();
    else cancel();
  };
  canvas.addEventListener("pointercancel", cancelActivePointer);
  canvas.addEventListener("lostpointercapture", cancelActivePointer);
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
      callback("zoom", x, y, Math.max(-120, Math.min(120, event.deltaY)), 0, false);
    },
    { passive: false },
  );
  window.addEventListener("keydown", (event) => {
    if (modalOpen) {
      const key = event.key.toLowerCase();
      if (key === "escape") {
        event.preventDefault();
        event.stopPropagation();
        callback("dismissModal", 0, 0, 0, 0, false);
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
  const resize = new ResizeObserver(() => callback("resize", 0, 0, 0, 0, false));
  resize.observe(canvas);
}
