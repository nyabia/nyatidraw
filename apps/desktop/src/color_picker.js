// HSV preview is local UI state; one confirmed RGBA selection becomes a command.
window.__nyatidrawColorDispose?.();
let resolveLifetime;
const lifetime = new Promise(resolve => { resolveLifetime = resolve; });
const root = document.getElementById('color-picker');
if (!root) throw Error('Color picker missing');
const controller = new AbortController();
const options = { capture: true, signal: controller.signal };
const wheel = root.querySelector('.color-wheel');
const square = root.querySelector('.sv-square');
const value = root.querySelector('.value-strip');
let hsv = [0, 0, 0];
let alpha = 255;
let drag = null;
const clamp = x => Math.max(0, Math.min(1, x));
const revision = () => document.querySelector('[data-dock-revision]')?.dataset.dockRevision;
function rgbToHsv([r, g, b]) {
    r /= 255; g /= 255; b /= 255;
    const max = Math.max(r, g, b), min = Math.min(r, g, b), delta = max - min;
    let h = hsv[0]; // Retain hue when selecting gray or black.
    if (delta) {
        h = max === r ? (g - b) / delta : max === g ? (b - r) / delta + 2 : (r - g) / delta + 4;
        h = ((h * 60) % 360 + 360) % 360;
    }
    return [h, max ? delta / max : hsv[1], max];
}
function rgba() {
    const [h, s, v] = hsv;
    const c = v * s, x = c * (1 - Math.abs((h / 60) % 2 - 1)), m = v - c;
    const sector = Math.floor(h / 60) % 6;
    const rgb = [[c,x,0],[x,c,0],[0,c,x],[0,x,c],[x,0,c],[c,0,x]][sector];
    return [...rgb.map(channel => Math.round((channel + m) * 255)), alpha];
}
function render() {
    const [h, s, v] = hsv;
    root.style.setProperty('--hue-color', `hsl(${h} 100% 50%)`);
    root.style.setProperty('--wheel-x', `${50 + Math.sin(h * Math.PI / 180) * 43}%`);
    root.style.setProperty('--wheel-y', `${50 - Math.cos(h * Math.PI / 180) * 43}%`);
    root.style.setProperty('--saturation', `${s * 100}%`);
    root.style.setProperty('--darkness', `${(1 - v) * 100}%`);
    root.style.setProperty('--value-top', `hsl(${h} 100% ${100 - s * 50}%)`);
    wheel.setAttribute('aria-valuenow', Math.round(h));
    square.setAttribute('aria-label', `채도 ${Math.round(s * 100)}%, 명도 ${Math.round(v * 100)}%`);
    value.setAttribute('aria-valuenow', Math.round(v * 100));
}
function restore() {
    const color = root.dataset.colorRgba.split(',').map(Number);
    hsv = rgbToHsv(color);
    alpha = color[3];
    render();
}
function commit(basedOn) {
    const selected = rgba().join(',');
    restore(); // Display authority again until the accepted command is projected.
    if (basedOn && basedOn === revision() && selected !== root.dataset.colorRgba) {
        dioxus.send([selected, basedOn]);
    }
}
function updatePointer(event) {
    const rect = drag.element.getBoundingClientRect();
    if (drag.axis === 'h') {
        hsv[0] = (Math.atan2(event.clientX - (rect.left + rect.width / 2),
            (rect.top + rect.height / 2) - event.clientY) * 180 / Math.PI + 360) % 360;
    } else if (drag.axis === 'sv') {
        hsv[1] = clamp((event.clientX - rect.left) / rect.width);
        hsv[2] = 1 - clamp((event.clientY - rect.top) / rect.height);
    } else {
        hsv[2] = 1 - clamp((event.clientY - rect.top) / rect.height);
    }
    render();
}
function finish(accepted) {
    if (!drag) return;
    const ended = drag;
    drag = null;
    if (ended.element.hasPointerCapture(ended.id)) ended.element.releasePointerCapture(ended.id);
    if (accepted) commit(ended.revision);
    else { hsv = ended.hsv; restore(); }
}
root.addEventListener('pointerdown', event => {
    const element = event.target.closest('[data-color-axis]');
    if (!element || drag || !event.isPrimary || event.button !== 0) return;
    const axis = element.dataset.colorAxis;
    if (axis === 'h') {
        const rect = element.getBoundingClientRect();
        const radius = Math.hypot(event.clientX - rect.left - rect.width / 2,
            event.clientY - rect.top - rect.height / 2);
        if (radius < rect.width * .35) return;
    }
    element.focus();
    try { element.setPointerCapture(event.pointerId); } catch { return; }
    if (!element.hasPointerCapture(event.pointerId)) return;
    drag = { element, axis, id: event.pointerId, revision: revision(), hsv: [...hsv] };
    event.preventDefault();
    event.stopImmediatePropagation();
    updatePointer(event);
}, options);
document.addEventListener('pointermove', event => {
    if (!drag || event.pointerId !== drag.id) return;
    if (!(event.buttons & 1) || !root.isConnected) { finish(false); return; }
    event.preventDefault(); event.stopImmediatePropagation();
    updatePointer(event);
}, options);
document.addEventListener('pointerup', event => {
    if (!drag || event.pointerId !== drag.id) return;
    updatePointer(event);
    finish(true);
    event.preventDefault(); event.stopImmediatePropagation();
}, options);
for (const name of ['pointercancel', 'lostpointercapture']) {
    document.addEventListener(name, event => {
        if (drag?.id === event.pointerId) finish(false);
    }, options);
}
document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && drag) {
        finish(false); event.preventDefault(); event.stopImmediatePropagation(); return;
    }
    const element = event.target.closest?.('[data-color-axis]');
    if (!element || !root.contains(element) || drag) return;
    const axis = element.dataset.colorAxis;
    const key = event.key;
    if (!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(key)) return;
    const step = event.shiftKey ? 10 : 1;
    const direction = key === 'ArrowRight' || key === 'ArrowUp' ? 1 : -1;
    if (axis === 'h') {
        hsv[0] = key === 'Home' ? 0 : key === 'End' ? 359 : (hsv[0] + direction * step + 360) % 360;
    } else {
        const index = axis === 'v' || key === 'ArrowUp' || key === 'ArrowDown' ? 2 : 1;
        hsv[index] = key === 'Home' ? 0 : key === 'End' ? 1 : clamp(hsv[index] + direction * step / 100);
    }
    render(); commit(revision());
    event.preventDefault(); event.stopImmediatePropagation();
}, options);
window.addEventListener('blur', event => { if (event.target === window) finish(false); }, options);
document.addEventListener('visibilitychange', () => { if (document.hidden) finish(false); }, options);
const observer = new MutationObserver(() => { finish(false); restore(); });
observer.observe(root, { attributes: true, attributeFilter: ['data-color-rgba'] });
window.__nyatidrawColorDispose = () => { finish(false); observer.disconnect(); controller.abort(); resolveLifetime(); };
restore();
await lifetime;
