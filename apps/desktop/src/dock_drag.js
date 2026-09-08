// Pointer coordinates stay inside the WebView. Only capture/cancel telemetry
// and the completed revision-bound semantic command cross the UI bridge.
window.__nyatidrawDockDispose?.();
const listeners = new AbortController();
const options = { capture: true, signal: listeners.signal };
const toolbars = new Set(['canvas-actions', 'viewport', 'quick-colors']);
const cancelProbes = __DOCK_CANCEL_PROBE__ ? ['escape', 'blur', 'pointercancel', 'lostpointercapture'] : [];
let drag = null;
let resize = null;
let suppressClick = false;
let clickTimer = null;
const marker = document.createElement('div');
marker.className = 'dock-capture-marker';
marker.setAttribute('aria-hidden', 'true');
document.body.append(marker);

function packet(source, target, action, revision) {
    dioxus.send([source, target, action, revision]);
}
function dockRevision() {
    return document.querySelector('[data-dock-revision]')?.dataset.dockRevision;
}
function resizeState(handle) {
    const first = handle.previousElementSibling;
    if (handle.dataset.resizeAxis === 'height') {
        const rect = first.getBoundingClientRect();
        const stack = handle.parentElement;
        const trailing = stack.lastElementChild.getBoundingClientRect();
        const low = 64;
        // Only this panel and the final filler change. Other leading heights stay fixed.
        const high = Math.min(4096, Math.max(low, Math.floor(rect.height + trailing.height - 160)));
        return { handle, rect, low, high, revision: dockRevision(),
            ratio: Math.round(rect.height), vertical: true, action: 'height' };
    }
    const second = handle.nextElementSibling;
    const rect = handle.parentElement.getBoundingClientRect();
    const extent = first.getBoundingClientRect().width + second.getBoundingClientRect().width;
    const low = Math.max(20, Math.ceil(Number(handle.dataset.firstMin) / extent * 1000));
    const high = Math.min(980, Math.floor((extent - Number(handle.dataset.secondMin)) / extent * 1000));
    if (!(extent > 0) || low > high) return null;
    return { handle, rect, extent, low, high, revision: dockRevision(), action: 'resize',
        ratio: Math.round(first.getBoundingClientRect().width / extent * 1000) };
}
function resizeRatio(state, x) {
    return Math.max(state.low, Math.min(state.high, Math.round((x - state.rect.left) / state.extent * 1000)));
}
function pointerValue(state, event) {
    return state.vertical
        ? Math.max(state.low, Math.min(state.high, Math.round(state.startRatio + event.clientY - state.startY)))
        : resizeRatio(state, state.rect.left + state.extent * state.startRatio / 1000 + event.clientX - state.startX);
}
function finishResize(commit = false) {
    const finished = resize;
    if (!finished) return;
    resize = null;
    marker.style.display = 'none';
    document.documentElement.classList.remove('dock-resizing');
    document.documentElement.classList.remove('dock-resizing-height');
    if (finished.handle.hasPointerCapture(finished.id)) finished.handle.releasePointerCapture(finished.id);
    // Preview is only a guide: cancellation leaves both DOM and persisted layout untouched.
    if (commit && finished.handle.isConnected && dockRevision() === finished.revision) {
        packet(finished.handle.dataset.dockResize, String(finished.ratio), finished.action, finished.revision);
    }
}
document.addEventListener('pointerdown', event => {
    if (drag || resize || !event.isPrimary || event.button !== 0) return;
    const handle = event.target.closest('[data-dock-resize]');
    if (!handle) return;
    const state = resizeState(handle);
    if (!state?.revision) return;
    try {
        handle.setPointerCapture(event.pointerId);
        if (!handle.hasPointerCapture(event.pointerId)) return;
    } catch { return; }
    resize = { ...state, id: event.pointerId, startX: event.clientX, startY: event.clientY, startRatio: state.ratio };
    document.documentElement.classList.add('dock-resizing');
    if (state.vertical) document.documentElement.classList.add('dock-resizing-height');
    event.preventDefault();
    event.stopImmediatePropagation();
}, options);
document.addEventListener('pointermove', event => {
    if (!resize || resize.id !== event.pointerId) return;
    if (!(event.buttons & 1) || !resize.handle.isConnected || dockRevision() !== resize.revision) {
        finishResize(); return;
    }
    resize.ratio = pointerValue(resize, event);
    marker.className = `dock-capture-marker ${resize.vertical ? 'horizontal' : 'vertical'}`;
    Object.assign(marker.style, resize.vertical
        ? { display: 'block', left: `${resize.rect.left}px`, top: `${resize.rect.top + resize.ratio}px`,
            width: `${resize.rect.width}px`, height: '5px' }
        : { display: 'block', left: `${resize.rect.left + resize.extent * resize.ratio / 1000}px`,
            top: `${resize.rect.top}px`, width: '5px', height: `${resize.rect.height}px` });
    event.preventDefault();
    event.stopImmediatePropagation();
}, options);
document.addEventListener('pointerup', event => {
    if (!resize || resize.id !== event.pointerId) return;
    resize.ratio = pointerValue(resize, event);
    finishResize(Math.abs(resize.vertical ? event.clientY - resize.startY : event.clientX - resize.startX) >= 1);
    event.preventDefault();
    event.stopImmediatePropagation();
}, options);
document.addEventListener('keydown', event => {
    if (resize && event.key === 'Escape') {
        finishResize(); event.preventDefault(); event.stopImmediatePropagation(); return;
    }
    const handle = event.target.closest('[data-dock-resize]');
    if (!handle || drag || resize) return;
    const arrows = handle.dataset.resizeAxis === 'height' ? ['ArrowUp', 'ArrowDown'] : ['ArrowLeft', 'ArrowRight'];
    if (![...arrows, 'Home', 'End'].includes(event.key)) return;
    const state = resizeState(handle);
    if (!state?.revision) return;
    const ratio = event.key === 'Home' ? state.low : event.key === 'End' ? state.high
        : Math.max(state.low, Math.min(state.high, state.ratio + (event.key === arrows[0] ? -10 : 10)));
    packet(handle.dataset.dockResize, String(ratio), state.action, state.revision);
    event.preventDefault();
    event.stopImmediatePropagation();
}, options);
function inside(rect, x, y) {
    return x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
}
function hit(x, y) {
    if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return null;
    if (toolbars.has(drag.source)) {
        for (const element of document.querySelectorAll('[data-dock-top]')) {
            const target = element.dataset.dockTop;
            const rect = element.getBoundingClientRect();
            if (target !== drag.source && inside(rect, x, y)) {
                return { target, action: 'top-row', rect, edge: 'left' };
            }
        }
    }
    for (const element of document.querySelectorAll('[data-dock-panel]')) {
        const target = element.dataset.dockPanel;
        if (target === drag.source) continue;
        const rect = element.getBoundingClientRect();
        if (!inside(rect, x, y)) continue;
        const rx = (x - rect.left) / rect.width;
        const ry = (y - rect.top) / rect.height;
        const edge = ry < .26 && rx >= .28 && rx <= .72 ? 'top'
            : ry >= .24 && ry <= .76 && rx < .28 ? 'left'
            : ry >= .24 && ry <= .76 && rx > .72 ? 'right' : null;
        if (edge) return { target, action: edge, rect, edge };
    }
    return null;
}
function show(target) {
    marker.style.display = target ? 'block' : 'none';
    if (!target) return;
    const { rect, edge } = target;
    const horizontal = edge === 'top';
    marker.className = `dock-capture-marker ${horizontal ? 'horizontal' : 'vertical'}`;
    Object.assign(marker.style, {
        left: `${edge === 'right' ? rect.right - 7 : rect.left + (horizontal ? 6 : 2)}px`,
        top: `${rect.top + (horizontal ? 2 : 6)}px`,
        width: `${horizontal ? Math.max(5, rect.width - 12) : 5}px`,
        height: `${horizontal ? 5 : Math.max(5, rect.height - 12)}px`,
    });
}
function finish(reason, target = null) {
    const finished = drag;
    if (!finished) return;
    drag = null;
    marker.style.display = 'none';
    document.documentElement.classList.remove('dock-capturing');
    finished.handle.classList.remove('dock-drag-source');
    if (finished.handle.hasPointerCapture(finished.id)) {
        finished.handle.releasePointerCapture(finished.id);
    }
    if (finished.moved) {
        suppressClick = true;
        clearTimeout(clickTimer);
        clickTimer = setTimeout(() => { suppressClick = false; }, 0);
    }
    if (target) packet(finished.source, target.target, target.action, finished.revision);
    else packet(finished.source, reason, 'cancel', finished.revision);
}
document.addEventListener('pointerdown', event => {
    if (drag || resize || !event.isPrimary || event.button !== 0) return;
    const handle = event.target.closest('[data-dock-source]');
    if (!handle) return;
    const revision = document.querySelector('[data-dock-revision]')?.dataset.dockRevision;
    if (!revision) return;
    try {
        handle.setPointerCapture(event.pointerId);
        if (!handle.hasPointerCapture(event.pointerId)) return;
    } catch { return; }
    drag = { handle, source: handle.dataset.dockSource, revision,
        id: event.pointerId, x: event.clientX, y: event.clientY, moved: false };
    packet(drag.source, 'held', 'capture', revision);
}, options);
document.addEventListener('pointermove', event => {
    if (!drag || event.pointerId !== drag.id) return;
    if (!(event.buttons & 1)) { finish('buttons-released'); return; }
    if (!drag.handle.isConnected) { finish('source-removed'); return; }
    if (!drag.moved && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) >= 4) {
        drag.moved = true;
        drag.handle.classList.add('dock-drag-source');
        document.documentElement.classList.add('dock-capturing');
    }
    if (drag.moved) {
        event.preventDefault();
        event.stopImmediatePropagation();
        show(hit(event.clientX, event.clientY));
        const probe = cancelProbes.shift();
        if (probe) {
            packet(drag.source, probe, 'synthetic-cancel-probe', drag.revision);
            if (probe === 'escape') document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
            else if (probe === 'blur') window.dispatchEvent(new Event('blur'));
            else if (probe === 'pointercancel') drag.handle.dispatchEvent(new PointerEvent('pointercancel', { pointerId: drag.id, bubbles: true }));
            else drag.handle.releasePointerCapture(drag.id);
        }
    }
}, options);
document.addEventListener('pointerup', event => {
    if (!drag || event.pointerId !== drag.id) return;
    const moved = drag.moved;
    // Recompute from release coordinates; never commit an earlier hover.
    const target = moved ? hit(event.clientX, event.clientY) : null;
    finish(moved ? 'outside-target' : 'click', target);
    if (moved) { event.preventDefault(); event.stopImmediatePropagation(); }
}, options);
for (const name of ['pointercancel', 'lostpointercapture']) {
    document.addEventListener(name, event => {
        if (resize?.id === event.pointerId) finishResize();
        if (drag?.id === event.pointerId) finish(name);
    }, options);
}
document.addEventListener('keydown', event => {
    if (drag && event.key === 'Escape') {
        finish('escape');
        event.preventDefault();
        event.stopImmediatePropagation();
    }
}, options);
document.addEventListener('click', event => {
    if (suppressClick) { event.preventDefault(); event.stopImmediatePropagation(); }
}, options);
document.addEventListener('dragstart', event => {
    if (drag) event.preventDefault();
}, options);
window.addEventListener('blur', event => {
    // Element focus changes also traverse window during the capture phase.
    if (event.target === window) { finish('window-blur'); finishResize(); }
}, options);
document.addEventListener('visibilitychange', () => {
    if (document.hidden) { finish('hidden'); finishResize(); }
}, options);
window.addEventListener('resize', () => finishResize(), options);
const observer = new MutationObserver(records => {
    if (drag && !drag.handle.isConnected) finish('source-removed');
    if (resize && (!resize.handle.isConnected || dockRevision() !== resize.revision)) finishResize();
    if (records.some(record => record.type === 'childList' || record.attributeName === 'data-dock-revision')) {
        requestAnimationFrame(() => window.dispatchEvent(new Event('nyatidraw-layout-changed')));
    }
});
observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-dock-revision'] });
window.__nyatidrawDockDispose = () => {
    finish('unmounted');
    finishResize();
    listeners.abort();
    observer.disconnect();
    marker.remove();
    clearTimeout(clickTimer);
};
await new Promise(() => {});
