import { openNyatiPopup } from "../nyatidraw.js";

const EDITOR_URL = "https://nyabia.github.io/nyatidraw/draw/";
const storageKey = `nyatidraw:composer:v1:${location.origin}`;
const records = new Set();
let profile = null;
let observer = null;
let reconcileScheduled = false;
let activeSession = null;
let resultBlob = null;
let resultUrl = null;
let resultPanel = null;
let resultTextarea = null;
let noticeSurface = null;
let settingsHost = null;

async function getStoredProfile() {
  if (typeof GM !== "undefined" && typeof GM.getValue === "function") return GM.getValue(storageKey, null);
  if (typeof GM_getValue === "function") return GM_getValue(storageKey, null);
  throw new Error("사용자 스크립트 관리자에 GM 저장소 기능이 필요합니다.");
}

async function setStoredProfile(value) {
  if (typeof GM !== "undefined" && typeof GM.setValue === "function") return GM.setValue(storageKey, value);
  if (typeof GM_setValue === "function") return GM_setValue(storageKey, value);
  throw new Error("사용자 스크립트 관리자에 GM 저장소 기능이 필요합니다.");
}

function registerMenu(name, callback) {
  if (typeof GM !== "undefined" && typeof GM.registerMenuCommand === "function") {
    GM.registerMenuCommand(name, callback);
  } else if (typeof GM_registerMenuCommand === "function") {
    GM_registerMenuCommand(name, callback);
  } else {
    console.warn("NyatiDraw: 사용자 스크립트 설정 메뉴를 등록할 수 없습니다.");
  }
}

function validSelector(value) {
  if (typeof value !== "string" || !value.trim()) return false;
  try { document.querySelectorAll(value); return true; }
  catch { return false; }
}

function validateProfile(value) {
  if (!value || typeof value !== "object" || value.origin !== location.origin || !Array.isArray(value.rules)) return null;
  if (value.rules.length < 1 || value.rules.length > 20) return null;
  const rules = [];
  for (const rule of value.rules) {
    if (!rule || !validSelector(rule.scopeSelector) || !validSelector(rule.textareaSelector)
        || !validSelector(rule.buttonAnchorSelector)) return null;
    rules.push({
      scopeSelector: rule.scopeSelector.trim(),
      textareaSelector: rule.textareaSelector.trim(),
      buttonAnchorSelector: rule.buttonAnchorSelector.trim(),
    });
  }
  return { origin: value.origin, rules };
}

function createSurface() {
  const host = document.createElement("div");
  host.setAttribute("data-nyatidraw-userscript-ui", "");
  host.style.cssText = "all:initial;position:fixed;z-index:2147483647;inset:0;pointer-events:none";
  const root = host.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = `
    * { box-sizing: border-box; }
    .box { color:#172028; background:#fff; border:1px solid #83919d; border-radius:10px;
      box-shadow:0 12px 40px #0005; font:14px/1.45 system-ui,sans-serif; pointer-events:auto; }
    .settings { position:absolute; inset:5vh auto auto 50%; transform:translateX(-50%);
      width:min(620px,94vw); max-height:90vh; overflow:auto; padding:18px; }
    .result { position:absolute; right:12px; bottom:12px; width:min(300px,calc(100vw - 24px)); padding:14px; }
    h2 { font-size:18px; margin:0 0 10px; } p { margin:8px 0; } label { display:block; margin:8px 0; }
    input { display:block; width:100%; padding:7px; border:1px solid #889; border-radius:5px; font:inherit; }
    input[readonly] { background:#eee; } .rule { padding:10px; margin:12px 0; border:1px solid #ccd; border-radius:7px; }
    .actions { display:flex; flex-wrap:wrap; gap:6px; margin-top:12px; }
    button, a.button { display:inline-block; padding:7px 10px; border:1px solid #677887; border-radius:6px;
      background:#f5f8fa; color:#172028; font:inherit; text-decoration:none; cursor:pointer; }
    button:focus-visible, a.button:focus-visible, input:focus-visible { outline:2px solid #1857bd; }
    img { display:block; max-width:100%; max-height:180px; margin:8px auto; object-fit:contain; }
    .error { color:#a21b1b; white-space:pre-wrap; } .muted { color:#445462; }
  `;
  root.append(style);
  document.body.append(host);
  return { host, root };
}

function field(labelText, value, readOnly = false) {
  const label = document.createElement("label");
  label.textContent = labelText;
  const input = document.createElement("input");
  input.type = "text";
  input.value = value;
  input.readOnly = readOnly;
  label.append(input);
  return { label, input };
}

function button(text, callback) {
  const item = document.createElement("button");
  item.type = "button";
  item.textContent = text;
  item.addEventListener("click", callback);
  return item;
}

function openSettings() {
  if (!document.body) return;
  if (settingsHost?.isConnected) return;
  const { host, root } = createSurface();
  settingsHost = host;
  const closeSettings = () => { host.remove(); settingsHost = null; };
  const box = document.createElement("section");
  box.className = "box settings";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", "NyatiDraw 사이트 설정");
  const title = document.createElement("h2");
  title.textContent = "NyatiDraw 사이트 설정";
  const help = document.createElement("p");
  help.textContent = "각 규칙은 작성창 범위 안에서 textarea와 버튼 위치를 하나씩 찾습니다. 범위 선택자는 여러 작성창을 가리킬 수 있습니다. 버튼은 지정한 위치 요소 뒤에 놓입니다.";
  const origin = field("현재 HTTPS origin (정확히 일치)", location.origin, true);
  const rulesHost = document.createElement("div");
  const message = document.createElement("p");
  message.setAttribute("role", "status");
  const actions = document.createElement("div");
  actions.className = "actions";
  const editors = [];

  function addRule(rule = { scopeSelector: "", textareaSelector: "", buttonAnchorSelector: "" }) {
    if (editors.length >= 20) { message.textContent = "규칙은 최대 20개까지 추가할 수 있습니다."; return; }
    const card = document.createElement("div");
    card.className = "rule";
    const scope = field("작성창 범위 CSS 선택자", rule.scopeSelector);
    const textarea = field("범위 안의 textarea CSS 선택자", rule.textareaSelector);
    const anchor = field("범위 안의 버튼 위치 CSS 선택자", rule.buttonAnchorSelector);
    card.append(scope.label, textarea.label, anchor.label);
    card.append(button("규칙 삭제", () => {
      card.remove();
      editors.splice(editors.findIndex(item => item.card === card), 1);
    }));
    rulesHost.append(card);
    editors.push({ card, scope: scope.input, textarea: textarea.input, anchor: anchor.input });
  }

  actions.append(
    button("규칙 추가", () => addRule()),
    button("저장하고 사용", async () => {
      message.className = "";
      if (location.protocol !== "https:" || origin.input.value !== location.origin || editors.length === 0) {
        message.textContent = "현재 페이지의 정확한 HTTPS origin과 규칙 하나 이상이 필요합니다.";
        message.className = "error";
        return;
      }
      const candidate = validateProfile({ origin: origin.input.value, rules: editors.map(item => ({
        scopeSelector: item.scope.value,
        textareaSelector: item.textarea.value,
        buttonAnchorSelector: item.anchor.value,
      })) });
      if (!candidate) {
        message.textContent = "모든 규칙의 세 선택자에 유효한 CSS 선택자를 입력하세요.";
        message.className = "error";
        return;
      }
      try {
        await setStoredProfile(candidate);
        activate(candidate);
        closeSettings();
      } catch (error) {
        message.textContent = `설정을 저장하지 못했습니다: ${error.message}`;
        message.className = "error";
      }
    }),
    button("이 사이트에서 사용 안 함", async () => {
      try {
        await setStoredProfile(null);
        activate(null);
        closeSettings();
      } catch (error) {
        message.textContent = `설정을 변경하지 못했습니다: ${error.message}`;
        message.className = "error";
      }
    }),
    button("닫기", closeSettings),
  );
  box.append(title, help, origin.label, rulesHost, message, actions);
  root.append(box);
  for (const rule of profile?.rules ?? [{ scopeSelector: "", textareaSelector: "", buttonAnchorSelector: "" }]) addRule(rule);
  editors[0]?.scope.focus();
}

function status(text) {
  if (resultPanel) {
    resultPanel.status.textContent = text;
    return;
  }
  if (!noticeSurface) {
    const { host, root } = createSurface();
    const box = document.createElement("section");
    box.className = "box result";
    box.setAttribute("role", "status");
    const message = document.createElement("p");
    box.append(message, button("닫기", () => { host.remove(); noticeSurface = null; }));
    root.append(box);
    noticeSurface = { host, message };
  }
  noticeSurface.message.textContent = text;
}

function showResult(result, textarea) {
  noticeSurface?.host.remove();
  noticeSurface = null;
  if (!resultPanel) {
    const { host, root } = createSurface();
    const box = document.createElement("section");
    box.className = "box result";
    box.setAttribute("role", "region");
    box.setAttribute("aria-label", "NyatiDraw PNG 결과");
    const title = document.createElement("h2");
    title.textContent = "NyatiDraw PNG";
    const preview = document.createElement("img");
    preview.alt = "그림 미리보기";
    const details = document.createElement("p");
    details.className = "muted";
    const notice = document.createElement("p");
    notice.textContent = "PNG를 복사한 뒤 작성창에 직접 붙여넣으세요. 붙여넣기를 지원하지 않는 사이트에서는 파일을 내려받아 직접 첨부하세요.";
    const state = document.createElement("p");
    state.setAttribute("role", "status");
    const actions = document.createElement("div");
    actions.className = "actions";
    const download = document.createElement("a");
    download.className = "button";
    download.textContent = "PNG 다운로드";
    download.download = "nyatidraw.png";
    actions.append(
      button("PNG 복사", async () => {
        try {
          if (!resultBlob || !navigator.clipboard?.write || typeof ClipboardItem === "undefined") throw new Error("이 브라우저에서 PNG 클립보드 쓰기를 사용할 수 없습니다.");
          await navigator.clipboard.write([new ClipboardItem({ "image/png": resultBlob })]);
          if (!resultTextarea?.isConnected || resultTextarea.disabled || resultTextarea.readOnly) {
            status("PNG를 복사했습니다. 원래 작성창을 사용할 수 없어 붙여넣을 작성창을 직접 선택하세요.");
          } else {
            try { resultTextarea.focus({ preventScroll: true }); }
            catch {
              status("PNG를 복사했습니다. 작성창을 직접 선택한 뒤 붙여넣으세요.");
              return;
            }
            status(document.activeElement === resultTextarea
              ? "PNG를 복사하고 작성창을 선택했습니다. 직접 붙여넣으세요."
              : "PNG를 복사했습니다. 작성창을 직접 선택한 뒤 붙여넣으세요.");
          }
        } catch (error) { status(`PNG 복사 실패: ${error.message} PNG 다운로드를 이용하세요.`); }
      }),
      download,
      button("닫기", () => {
        if (resultUrl) URL.revokeObjectURL(resultUrl);
        resultBlob = null;
        resultUrl = null;
        resultTextarea = null;
        host.remove();
        resultPanel = null;
      }),
    );
    box.append(title, preview, details, notice, state, actions);
    root.append(box);
    resultPanel = { host, preview, details, download, status: state };
  }
  const nextUrl = URL.createObjectURL(result.blob);
  if (resultUrl) URL.revokeObjectURL(resultUrl);
  resultBlob = result.blob;
  resultUrl = nextUrl;
  resultTextarea = textarea;
  resultPanel.preview.src = nextUrl;
  resultPanel.download.href = nextUrl;
  resultPanel.details.textContent = `${result.width} × ${result.height} · ${result.bytes.byteLength.toLocaleString()} bytes`;
  status("PNG를 받았습니다. 복사하거나 다운로드하세요.");
}

function openEditor(textarea) {
  if (!textarea.isConnected || textarea.disabled || textarea.readOnly) return;
  if (activeSession) { status("열려 있는 편집기를 먼저 완료하세요."); activeSession.window?.focus(); return; }
  try {
    let completed = false;
    const session = openNyatiPopup({
      editorUrl: EDITOR_URL,
      onComplete(result) { showResult(result, textarea); completed = true; },
      onCancel() { status("편집기에서 그림 작업을 취소했습니다."); },
      onError(error) { status(error.message); },
      onClose({ reason }) {
        if (activeSession === session) activeSession = null;
        if (reason === "window-closed" && !completed) status("PNG 결과 없이 편집기 창이 닫혔습니다.");
      },
    });
    activeSession = session;
    session.ready.catch(error => {
      if (activeSession === session) activeSession = null;
      status(error.message);
    });
  } catch (error) {
    status(`NyatiDraw를 열지 못했습니다: ${error.message}`);
  }
}

function removeRecord(record) {
  record.button.remove();
  records.delete(record);
}

function reconcile() {
  reconcileScheduled = false;
  if (!profile) return;
  const desired = [];
  profile.rules.forEach((rule, ruleIndex) => {
    for (const scope of document.querySelectorAll(rule.scopeSelector)) {
      const textareas = scope.querySelectorAll(rule.textareaSelector);
      const anchors = [...scope.querySelectorAll(rule.buttonAnchorSelector)]
        .filter(element => !element.hasAttribute("data-nyatidraw-composer-button"));
      if (textareas.length !== 1 || anchors.length !== 1 || textareas[0].tagName !== "TEXTAREA"
          || textareas[0].disabled || textareas[0].readOnly
          || !anchors[0].parentElement) continue;
      if (desired.some(item => item.anchor === anchors[0])) continue;
      desired.push({ ruleIndex, scope, textarea: textareas[0], anchor: anchors[0] });
    }
  });
  for (const record of [...records]) {
    if (!desired.some(item => item.ruleIndex === record.ruleIndex && item.scope === record.scope
        && item.textarea === record.textarea && item.anchor === record.anchor && record.button.isConnected)) removeRecord(record);
  }
  for (const item of desired) {
    if ([...records].some(record => record.ruleIndex === item.ruleIndex && record.scope === item.scope)) continue;
    const action = button("그림 그리기", () => openEditor(item.textarea));
    action.setAttribute("data-nyatidraw-composer-button", "");
    action.style.cssText = "display:inline-block;margin:4px;padding:6px 10px;border:1px solid #526c82;border-radius:6px;background:#f5f8ff;color:#172028;font:14px system-ui;cursor:pointer";
    item.anchor.insertAdjacentElement("afterend", action);
    records.add({ ...item, button: action });
  }
}

function scheduleReconcile() {
  if (reconcileScheduled) return;
  reconcileScheduled = true;
  requestAnimationFrame(reconcile);
}

function activate(nextProfile) {
  observer?.disconnect();
  observer = null;
  for (const record of [...records]) removeRecord(record);
  profile = nextProfile;
  if (!profile) return;
  reconcile();
  observer = new MutationObserver(scheduleReconcile);
  observer.observe(document.documentElement, { childList: true, subtree: true, attributes: true });
}

async function main() {
  if (location.protocol !== "https:") return;
  registerMenu("NyatiDraw: 이 사이트 설정", openSettings);
  try {
    activate(validateProfile(await getStoredProfile()));
  } catch (error) {
    console.warn("NyatiDraw 사이트 설정을 불러오지 못했습니다:", error);
  }
}

void main();
