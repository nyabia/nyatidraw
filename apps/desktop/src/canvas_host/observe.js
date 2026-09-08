// Only layout changes cross IPC. Raw mouse/pen events never enter this script.
const element = document.getElementById('shared-gpu-canvas');
if (!element) throw new Error('native canvas placeholder missing');
window.__nyatidrawHostDispose?.();
const listeners = new AbortController();
const options = { signal: listeners.signal };
let disposed = false;
let frame = 0;
document.documentElement.classList.add('native-composition');
let last = '';
let scheduled = false;
function publish() {
    scheduled = false;
    if (disposed) return;
    if (!element.isConnected) {
        dioxus.send(['missing', [0, 0, 0, 0, 1], false, []]);
        dispose();
        return;
    }
    const canvas = element.getBoundingClientRect();
    const regions = [];
    function visit(node) {
        if (!(node instanceof HTMLElement) || node === element) return;
        const style = getComputedStyle(node);
        if (style.display === 'none' || style.visibility === 'hidden') return;
        const r = node.getBoundingClientRect();
        if (!node.contains(element) && style.pointerEvents !== 'none' && r.width > 0 && r.height > 0
            && r.left < canvas.right && r.right > canvas.left && r.top < canvas.bottom && r.bottom > canvas.top) {
            regions.push([r.left, r.top, r.width, r.height]);
            return;
        }
        for (const child of node.children) visit(child);
    }
    visit(document.body);
    // Fail closed if pathological chrome produces too many regions.
    const bounded = regions.length > 128 ? [[0, 0, innerWidth, innerHeight]] : regions;
    const packet = ['geometry', [canvas.left, canvas.top, canvas.width, canvas.height, devicePixelRatio || 1], true, bounded];
    const serialized = JSON.stringify(packet);
    if (serialized !== last) { last = serialized; dioxus.send(packet); }
}
function schedule() {
    if (!disposed && !scheduled) { scheduled = true; frame = requestAnimationFrame(publish); }
}
const size = new ResizeObserver(schedule);
size.observe(element);
size.observe(document.documentElement);
const changes = new MutationObserver(schedule);
changes.observe(document.body, { subtree: true, childList: true, attributes: true,
    attributeFilter: ['style', 'class', 'hidden', 'open'] });
window.addEventListener('resize', schedule, options);
window.addEventListener('scroll', schedule, { ...options, capture: true });
window.addEventListener('nyatidraw-layout-changed', schedule, options);
window.addEventListener('transitionend', schedule, { ...options, capture: true });
function dispose() {
    disposed = true;
    cancelAnimationFrame(frame);
    listeners.abort();
    size.disconnect();
    changes.disconnect();
}
window.__nyatidrawHostDispose = dispose;
publish();
await new Promise(() => {});
