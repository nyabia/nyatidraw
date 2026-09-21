// Live pointer coordinates remain in the WebView. Only the final semantic drop
// crosses IPC; Rust rechecks revision, index adjustment and group-cycle safety.
window.__nyatidrawLayerPointerDispose?.();
const listeners = new AbortController();
const options = { capture: true, signal: listeners.signal };
let drag = null;
let frame = 0;
let suppressClick = false;
let clickTimer = null;
const marker = document.createElement('div');
marker.className = 'layer-pointer-marker';
marker.setAttribute('aria-hidden', 'true');
document.body.append(marker);
const revision = () => document.querySelector('[data-dock-revision]')?.dataset.dockRevision;
const rowFor = key => document.querySelector(`[data-layer-key="${key}"]`);

function hit(x, y) {
    if (!drag || revision() !== drag.revision) return null;
    const row = document.elementFromPoint(x, y)?.closest('[data-layer-key]');
    if (!row || row === drag.row || row.closest('.layer-list') !== drag.list) return null;
    const rect = row.getBoundingClientRect();
    const fraction = (y - rect.top) / rect.height;
    const group = row.dataset.layerKey.startsWith('g-');
    const position = group && fraction >= .25 && fraction <= .75 ? 'inside'
        : fraction < .5 ? 'above' : 'below';
    let parent = position === 'inside' ? row.dataset.layerKey.slice(2) : row.dataset.parent;
    let index = position === 'inside'
        ? Number.MAX_SAFE_INTEGER : Number(row.dataset.index) + (position === 'above' ? 1 : 0);
    // Check ancestry from all rendered ancestors; collapsed descendants cannot
    // be drop targets. Rust checks again against the complete authoritative tree.
    for (let guard = 0; guard <= drag.list.children.length; guard++) {
        if (drag.source === `g-${parent}`) return null;
        const ancestor = rowFor(`g-${parent}`);
        if (!ancestor) break;
        parent = ancestor.dataset.parent;
    }
    parent = position === 'inside' ? row.dataset.layerKey.slice(2) : row.dataset.parent;
    if (parent === drag.row.dataset.parent) {
        if (Number(drag.row.dataset.index) < index) index--;
        if (Number(drag.row.dataset.index) === index) return null;
    }
    return { row, rect, position };
}

function show(target) {
    marker.style.display = target ? 'block' : 'none';
    if (!target) return;
    const { rect, position } = target;
    marker.classList.toggle('inside', position === 'inside');
    Object.assign(marker.style, {
        left: `${rect.left}px`, width: `${rect.width}px`,
        top: `${position === 'below' ? rect.bottom - 2 : rect.top - 2}px`,
        height: `${position === 'inside' ? rect.height : 4}px`,
    });
}

function finish(commit = false) {
    const ended = drag;
    if (!ended) return;
    const target = commit && ended.moved && ended.row.isConnected ? hit(ended.x, ended.y) : null;
    drag = null;
    cancelAnimationFrame(frame);
    marker.style.display = 'none';
    ended.row.classList.remove('layer-pointer-source');
    document.documentElement.classList.remove('layer-pointer-dragging');
    if (ended.row.hasPointerCapture(ended.id)) ended.row.releasePointerCapture(ended.id);
    if (ended.moved) {
        suppressClick = true;
        clearTimeout(clickTimer);
        clickTimer = setTimeout(() => { suppressClick = false; }, 0);
    }
    if (target) dioxus.send([ended.source, target.row.dataset.layerKey, target.position, ended.revision]);
}

function tick() {
    if (!drag?.moved) return;
    if (!drag.row.isConnected || revision() !== drag.revision) { finish(); return; }
    const rect = drag.list.getBoundingClientRect();
    if (drag.x >= rect.left && drag.x <= rect.right && drag.y >= rect.top && drag.y <= rect.bottom) {
        const delta = drag.y < rect.top + 26 ? -8 : drag.y > rect.bottom - 26 ? 8 : 0;
        if (delta) drag.list.scrollTop += delta;
    }
    show(hit(drag.x, drag.y));
    frame = requestAnimationFrame(tick);
}

document.addEventListener('pointerdown', event => {
    if (drag || !event.isPrimary || event.button !== 0) return;
    const row = event.target.closest('[data-layer-key]');
    if (!row || event.target.closest('button, input:not([readonly]), textarea, select')) return;
    const rev = revision();
    if (!rev) return;
    // Capture starts at the movement threshold, preserving ordinary clicks and
    // double-click-to-rename without retargeting them to the row.
    drag = { row, list: row.closest('.layer-list'), source: row.dataset.layerKey,
        revision: rev, id: event.pointerId, x: event.clientX, y: event.clientY,
        startX: event.clientX, startY: event.clientY, moved: false };
}, options);
document.addEventListener('pointermove', event => {
    if (!drag || event.pointerId !== drag.id) return;
    if (!(event.buttons & 1) || !drag.row.isConnected || revision() !== drag.revision) { finish(); return; }
    drag.x = event.clientX; drag.y = event.clientY;
    if (!drag.moved && Math.hypot(drag.x - drag.startX, drag.y - drag.startY) >= 4) {
        try {
            drag.row.setPointerCapture(drag.id);
            if (!drag.row.hasPointerCapture(drag.id)) { finish(); return; }
        } catch { finish(); return; }
        drag.moved = true;
        drag.row.classList.add('layer-pointer-source');
        document.documentElement.classList.add('layer-pointer-dragging');
        window.getSelection()?.removeAllRanges();
        tick();
    }
    if (drag?.moved) { event.preventDefault(); event.stopImmediatePropagation(); }
}, options);
document.addEventListener('pointerup', event => {
    if (!drag || event.pointerId !== drag.id) return;
    const moved = drag.moved;
    drag.x = event.clientX; drag.y = event.clientY;
    finish(true);
    if (moved) { event.preventDefault(); event.stopImmediatePropagation(); }
}, options);
for (const name of ['pointercancel', 'lostpointercapture']) {
    document.addEventListener(name, event => { if (drag?.id === event.pointerId) finish(); }, options);
}
document.addEventListener('keydown', event => {
    if (drag && event.key === 'Escape') { finish(); event.preventDefault(); event.stopImmediatePropagation(); }
}, options);
document.addEventListener('dragstart', event => {
    if (event.target.closest('[data-layer-key]')) event.preventDefault();
}, options);
document.addEventListener('click', event => {
    if (suppressClick) { event.preventDefault(); event.stopImmediatePropagation(); }
}, options);
window.addEventListener('blur', event => { if (event.target === window) finish(); }, options);
document.addEventListener('visibilitychange', () => { if (document.hidden) finish(); }, options);
window.__nyatidrawLayerPointerDispose = () => {
    finish(); listeners.abort(); marker.remove(); clearTimeout(clickTimer);
};
await new Promise(() => {});
