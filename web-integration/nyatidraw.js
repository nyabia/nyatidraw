const CHANNEL = "nyatidraw";
const VERSION = 1;
const STARTUP_TIMEOUT_MS = 60000;
const ACCEPTANCE_TIMEOUT_MS = 12000;
const MAX_PNG_BYTES = 96 * 1024 * 1024;
const UI_KEYS = ["toolbar", "tools", "brush", "colors", "layers", "history", "navigator"];
const TOOLS = new Set(["pen", "pencil2h", "pencil2b", "brush", "eraser"]);
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

function editorUrl(value, mode, session) {
  const url = new URL(value ?? "../", import.meta.url);
  if ((url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)))
      || url.origin === "null" || url.username || url.password) {
    throw new Error("The editor URL must be HTTPS or a localhost development URL without credentials.");
  }
  if (location.origin === "null" || (location.protocol !== "https:"
      && !(location.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(location.hostname)))) {
    throw new Error("Serve the host page over HTTPS or localhost; file: pages cannot embed NyatiDraw.");
  }
  url.searchParams.set("nyatiMode", mode);
  url.searchParams.set("hostOrigin", location.origin);
  url.searchParams.set("session", session);
  return url;
}

function validateConfig(input = {}) {
  if (!input || typeof input !== "object" || Array.isArray(input)) throw new TypeError("config must be an object");
  const canvas = input.canvas ?? {};
  const ui = input.ui ?? {};
  if (!canvas || typeof canvas !== "object" || Array.isArray(canvas)) throw new TypeError("config.canvas must be an object");
  if (!ui || typeof ui !== "object" || Array.isArray(ui)) throw new TypeError("config.ui must be an object");
  const width = canvas.width ?? 1024;
  const height = canvas.height ?? 768;
  if (!Number.isInteger(width) || !Number.isInteger(height) || width < 1 || height < 1
      || width > 4096 || height > 4096 || width * height > 16777216) {
    throw new RangeError("Canvas must be 1–4096 px per side and at most 16,777,216 pixels.");
  }
  const tool = input.tool ?? "pen";
  const size = input.size ?? 10;
  const color = input.color ?? "#000000";
  const opacity = input.opacity ?? 1;
  if (!TOOLS.has(tool)) throw new RangeError("Unknown initial tool.");
  if (typeof size !== "number" || !Number.isFinite(size) || size < 0.1 || size > 200) throw new RangeError("Brush size must be 0.1–200.");
  if (typeof color !== "string" || !/^#[0-9a-fA-F]{6}$/.test(color)) throw new RangeError("Color must be #RRGGBB.");
  if (typeof opacity !== "number" || !Number.isFinite(opacity) || opacity < 0.01 || opacity > 1) throw new RangeError("Opacity must be 0.01–1.");
  for (const key of Object.keys(ui)) {
    if (!UI_KEYS.includes(key) || typeof ui[key] !== "boolean") throw new TypeError(`Unsupported UI setting: ${key}`);
  }
  return {
    canvas: { width, height }, tool, size, color, opacity,
    ui: Object.fromEntries(UI_KEYS.map(key => [key, ui[key] ?? true])),
  };
}

function checkedPng(message) {
  const { bytes, width, height, mime, requestId } = message;
  if (typeof requestId !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(requestId)
      || mime !== "image/png" || !(bytes instanceof ArrayBuffer)
      || bytes.byteLength < 33 || bytes.byteLength > MAX_PNG_BYTES
      || !Number.isInteger(width) || !Number.isInteger(height)
      || width < 1 || height < 1 || width > 4096 || height > 4096
      || width * height > 16777216) {
    throw new Error("Invalid PNG result metadata.");
  }
  const data = new Uint8Array(bytes);
  if (!PNG_SIGNATURE.every((value, index) => data[index] === value)
      || data[8] !== 0 || data[9] !== 0 || data[10] !== 0 || data[11] !== 13
      || data[12] !== 73 || data[13] !== 72 || data[14] !== 68 || data[15] !== 82) {
    throw new Error("Result is not a PNG with an IHDR header.");
  }
  const view = new DataView(bytes);
  if (view.getUint32(16) !== width || view.getUint32(20) !== height) {
    throw new Error("PNG dimensions do not match the result metadata.");
  }
  return { requestId, mime, width, height, bytes, blob: new Blob([bytes], { type: mime }) };
}

function acceptBeforeDeadline(callback) {
  const controller = new AbortController();
  let timer;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => {
      controller.abort();
      reject(new Error("Host acceptance timed out after 12 seconds. Try Done again or download from the editor."));
    }, ACCEPTANCE_TIMEOUT_MS);
  });
  return Promise.race([Promise.resolve().then(() => callback(controller.signal)), deadline])
    .finally(() => clearTimeout(timer));
}

function createSession({ mode, url, config, onComplete, onCancel, onError, onClose, attach }) {
  if (typeof onComplete !== "function") throw new TypeError("onComplete(result) is required to accept the PNG.");
  if (onCancel != null && typeof onCancel !== "function") throw new TypeError("onCancel must be a function.");
  if (onError != null && typeof onError !== "function") throw new TypeError("onError must be a function.");
  if (onClose != null && typeof onClose !== "function") throw new TypeError("onClose must be a function.");
  const session = [...crypto.getRandomValues(new Uint8Array(16))].map(x => x.toString(16).padStart(2, "0")).join("");
  const target = editorUrl(url, mode, session);
  const initial = validateConfig(config);
  const frameOrPopup = attach(target.href);
  if (!frameOrPopup) throw new Error("Popup blocked. Open it directly from a user click, and allow popups for this site.");
  const source = mode === "embed" ? frameOrPopup.contentWindow : frameOrPopup;
  if (!source) throw new Error("Could not access the editor window.");
  let state = "starting";
  let pendingRequestId = null;
  let acceptedRequestId = null;
  let busy = false;
  let settleReady;
  let failReady;
  const ready = new Promise((resolve, reject) => { settleReady = resolve; failReady = reject; });
  const report = (error) => {
    if (state === "starting") {
      state = "failed";
      cleanup();
      failReady(error);
    }
    onError?.(error);
  };
  const post = (message) => {
    try { source.postMessage({ channel: CHANNEL, version: VERSION, session, ...message }, target.origin); }
    catch (error) { report(new Error(`Editor message channel unavailable: ${error.message}`)); }
  };
  const startupTimer = setTimeout(() => {
    if (state === "starting") report(new Error("Editor did not become ready. Check its URL, WebGPU support, and popup/COOP policy. The editor remains open for a manual download."));
  }, STARTUP_TIMEOUT_MS);
  const popupPoll = mode === "popup" ? setInterval(() => {
    if (frameOrPopup.closed && state !== "closed") {
      const wasStarting = state === "starting";
      state = "closed";
      cleanup();
      if (wasStarting) failReady(new Error("Editor window closed or its opener was isolated before it became ready. Check popup and COOP policy."));
      onClose?.({ reason: "window-closed" });
    }
  }, 500) : null;

  function cleanup() {
    clearTimeout(startupTimer);
    if (popupPoll) clearInterval(popupPoll);
    window.removeEventListener("message", onMessage);
  }

  async function acceptResult(message) {
    if (busy || state !== "active") return;
    let result;
    try { result = checkedPng(message); }
    catch (error) {
      post({ type: "error", requestId: message.requestId, message: error.message });
      report(error);
      return;
    }
    if (result.requestId === acceptedRequestId) {
      post({ type: "ack", requestId: result.requestId });
      return;
    }
    busy = true;
    try {
      await acceptBeforeDeadline(signal => onComplete({ ...result, signal }));
      if (state !== "active") return;
      acceptedRequestId = result.requestId;
      pendingRequestId = result.requestId;
      post({ type: "ack", requestId: result.requestId });
    } catch (error) {
      if (state !== "active") return;
      const reason = error instanceof Error ? error.message : String(error);
      post({ type: "error", requestId: result.requestId, message: reason.slice(0, 500) });
      report(new Error(`Host did not accept the PNG: ${reason}`));
    } finally { busy = false; }
  }

  function onMessage(event) {
    const message = event.data;
    if (event.source !== source || event.origin !== target.origin || !message
        || typeof message !== "object" || message.channel !== CHANNEL
        || message.version !== VERSION || message.session !== session
        || typeof message.type !== "string") return;
    if (message.type === "ready" && state === "starting") {
      post({ type: "init", config: initial });
    } else if (message.type === "initialized" && state === "starting") {
      state = "active";
      clearTimeout(startupTimer);
      settleReady(handle);
    } else if (message.type === "complete") {
      void acceptResult(message);
    } else if (message.type === "cancel" && state === "active") {
      if (typeof message.requestId !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(message.requestId) || busy) return;
      busy = true;
      acceptBeforeDeadline(signal => onCancel?.({ signal })).then(() => {
        if (state !== "active") return;
        pendingRequestId = message.requestId;
        post({ type: "ack", requestId: message.requestId });
      }, error => {
        if (state !== "active") return;
        const reason = error instanceof Error ? error.message : String(error);
        post({ type: "error", requestId: message.requestId, message: reason.slice(0, 500) });
        report(new Error(`Host did not accept cancellation: ${reason}`));
      }).finally(() => { busy = false; });
    } else if (message.type === "closed" && state === "active" && message.requestId === pendingRequestId) {
      state = "closed";
      cleanup();
      if (mode === "embed") frameOrPopup.remove();
      onClose?.({ reason: message.requestId === acceptedRequestId ? "completed" : "cancelled" });
    } else if (message.type === "error") {
      const error = new Error(typeof message.message === "string" ? message.message : "Editor reported an error.");
      report(error);
    }
  }

  window.addEventListener("message", onMessage);
  const handle = {
    mode,
    element: mode === "embed" ? frameOrPopup : null,
    window: mode === "popup" ? frameOrPopup : null,
    ready,
    dispose() {
      if (state === "closed") return;
      const wasStarting = state === "starting";
      state = "closed";
      cleanup();
      if (mode === "embed") frameOrPopup.remove();
      else frameOrPopup.close();
      if (wasStarting) failReady(new Error("Editor session disposed before it became ready."));
      onClose?.({ reason: "disposed" });
    },
  };
  return handle;
}

export function mountNyatiIframe(container, options = {}) {
  if (!(container instanceof Element)) throw new TypeError("container must be a DOM element.");
  return createSession({
    ...options,
    mode: "embed",
    url: options.editorUrl,
    attach(href) {
      const iframe = document.createElement("iframe");
      iframe.title = "NyatiDraw editor";
      iframe.src = href;
      iframe.setAttribute("sandbox", "allow-scripts allow-same-origin allow-downloads");
      iframe.style.cssText = "display:block;width:100%;height:100%;border:0";
      container.append(iframe);
      return iframe;
    },
  });
}

export function openNyatiPopup(options = {}) {
  return createSession({
    ...options,
    mode: "popup",
    url: options.editorUrl,
    attach(href) {
      const popup = window.open(href, "_blank", "popup,width=1280,height=900,resizable=yes,scrollbars=yes");
      popup?.focus();
      return popup;
    },
  });
}
