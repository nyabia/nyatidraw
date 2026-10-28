window.__nyatidrawReferenceDispose?.();
let resolveLifetime;
const lifetime = new Promise(resolve => { resolveLifetime = resolve; });
const root = document.getElementById('reference-panel');
if (!root) throw Error('Reference panel missing');
const view = root.querySelector('.reference-view');
const preview = root.querySelector('.reference-color-preview');
const controller = new AbortController();
const options = { capture: true, signal: controller.signal };
const canvas = document.createElement('canvas');
canvas.width = canvas.height = 1;
const context = canvas.getContext('2d', { willReadFrequently: true });
if (context) context.imageSmoothingEnabled = false;
let drag = null;
const basis = () => [root.dataset.colorEpoch, root.dataset.colorRgba, root.dataset.referenceGeneration];
const unchanged = values => values.every((value, index) => value === basis()[index]);
const inside = (rect, x, y) => x >= rect.left && y >= rect.top && x < rect.right && y < rect.bottom;

function sample(event) {
    const image = view.querySelector('img');
    if (!context || !root.isConnected || !image?.complete || !image.naturalWidth
        || !inside(view.getBoundingClientRect(), event.clientX, event.clientY)) return null;
    // The transformed box still includes object-fit:contain letterboxing.
    const box = image.getBoundingClientRect();
    const scale = Math.min(box.width / image.naturalWidth, box.height / image.naturalHeight);
    if (!(scale > 0)) return null;
    const left = box.left + (box.width - image.naturalWidth * scale) / 2;
    const top = box.top + (box.height - image.naturalHeight * scale) / 2;
    const x = Math.floor((event.clientX - left) / scale);
    const y = Math.floor((event.clientY - top) / scale);
    if (x < 0 || y < 0 || x >= image.naturalWidth || y >= image.naturalHeight) return null;
    try {
        context.clearRect(0, 0, 1, 1);
        context.drawImage(image, x, y, 1, 1, 0, 0, 1, 1);
        const color = Array.from(context.getImageData(0, 0, 1, 1).data);
        return color[3] ? [color[0], color[1], color[2], 255] : null;
    } catch {
        return null;
    }
}

function show(color) {
    preview.hidden = !color;
    if (!color) return;
    const hex = `#${color.slice(0, 3).map(value => value.toString(16).padStart(2, '0')).join('')}`;
    preview.style.setProperty('--sample-color', hex);
    preview.textContent = hex.toUpperCase();
}

function finish(color = null) {
    const ended = drag;
    drag = null;
    show(null);
    if (!ended) return;
    if (view.hasPointerCapture(ended.id)) view.releasePointerCapture(ended.id);
    if (color && root.isConnected && root.dataset.referencePick === 'true'
        && unchanged(ended.basis) && color.join(',') !== ended.basis[1]) {
        dioxus.send([color.join(','), ...ended.basis]);
    }
}

document.addEventListener('pointerdown', event => {
    const contact = event.pointerType === 'pen' ? (event.buttons & 1) || event.pressure > 0 : event.button === 0;
    if (root.dataset.referencePick !== 'true' || !view.contains(event.target) || drag || !contact) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (!view.querySelector('img') || !context) return;
    try { view.setPointerCapture(event.pointerId); } catch { return; }
    if (!view.hasPointerCapture(event.pointerId)) return;
    drag = { id: event.pointerId, basis: basis() };
    show(sample(event));
}, options);
document.addEventListener('pointermove', event => {
    if (!drag || event.pointerId !== drag.id) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (!((event.buttons & 1) || (event.pointerType === 'pen' && event.pressure > 0))
        || !unchanged(drag.basis)) { finish(); return; }
    show(sample(event));
}, options);
document.addEventListener('pointerup', event => {
    if (!drag || event.pointerId !== drag.id) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    finish(sample(event));
}, options);
document.addEventListener('pointercancel', event => {
    if (drag?.id === event.pointerId) finish();
}, options);
view.addEventListener('lostpointercapture', event => {
    if (drag?.id === event.pointerId) finish();
}, options);
document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && drag) {
        finish();
        event.preventDefault();
        event.stopImmediatePropagation();
    }
}, options);
window.addEventListener('blur', event => { if (event.target === window) finish(); }, options);
window.addEventListener('resize', () => finish(), options);
document.addEventListener('visibilitychange', () => { if (document.hidden) finish(); }, options);
const stateObserver = new MutationObserver(() => finish());
stateObserver.observe(root, { attributes: true, attributeFilter: [
    'data-color-rgba', 'data-color-epoch', 'data-reference-generation', 'data-reference-pick'
] });
const removalObserver = new MutationObserver(() => { if (!root.isConnected) dispose(); });
removalObserver.observe(document.body, { childList: true, subtree: true });
function dispose() {
    finish();
    stateObserver.disconnect();
    removalObserver.disconnect();
    controller.abort();
    resolveLifetime();
}
window.__nyatidrawReferenceDispose = dispose;
await lifetime;
