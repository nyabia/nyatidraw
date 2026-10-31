const root = document.querySelector('main.app-shell');
const help = document.getElementById('editor-control-help');
if (!root || !help) return;
window.__nyatiControlHelpCleanup?.();
const controller = new AbortController();
const signal = controller.signal;
const selector = 'button,input,select,summary,[role="button"],[role="slider"],[data-color-axis],[data-dock-source]';
let current = null;
let timer = null;
let titles = [];
let accessibleName = null;

function hide() {
    clearTimeout(timer);
    timer = null;
    help.hidden = true;
    if (accessibleName && current?.getAttribute('aria-label') === accessibleName) current.removeAttribute('aria-label');
    accessibleName = null;
    if (current) {
        const ids = (current.getAttribute('aria-describedby') || '').split(/\s+/).filter(id => id && id !== help.id);
        if (ids.length) current.setAttribute('aria-describedby', ids.join(' '));
        else current.removeAttribute('aria-describedby');
    }
    for (const [node, title] of titles) {
        if (!node.hasAttribute('title')) node.setAttribute('title', title);
    }
    titles = [];
    current = null;
}

function targetFor(event) {
    const node = event.target instanceof Element ? event.target.closest(selector) : null;
    return node && root.contains(node) && !node.closest('[data-no-tooltip]') ? node : null;
}

function describe(node) {
    return node.getAttribute('data-help') || node.getAttribute('title')
        || node.closest('label[title]')?.getAttribute('title')
        || node.getAttribute('aria-label') || '';
}

function show(node, text) {
    if (!node.isConnected) return hide();
    const bounds = node.getBoundingClientRect();
    if (!bounds.width || !bounds.height) return hide();
    help.textContent = text;
    help.hidden = false;
    help.style.left = '8px';
    help.style.top = '8px';
    const rect = help.getBoundingClientRect();
    const left = Math.max(8, Math.min(innerWidth - rect.width - 8, bounds.left + (bounds.width - rect.width) / 2));
    const above = bounds.top - rect.height - 7;
    const top = above >= 8 ? above : Math.max(8, Math.min(innerHeight - rect.height - 8, bounds.bottom + 7));
    help.style.left = `${left}px`;
    help.style.top = `${top}px`;
    const ids = new Set((node.getAttribute('aria-describedby') || '').split(/\s+/).filter(Boolean));
    ids.add(help.id);
    node.setAttribute('aria-describedby', [...ids].join(' '));
}

function schedule(node, delay) {
    if (node === current) return;
    hide();
    if (!node) return;
    const text = describe(node).trim();
    if (!text) return;
    current = node;
    if (node.hasAttribute('title') && !node.hasAttribute('aria-label') && !node.hasAttribute('aria-labelledby')) {
        accessibleName = text;
        node.setAttribute('aria-label', text);
    }
    for (let parent = node; parent && parent !== root; parent = parent.parentElement) {
        if (parent.hasAttribute('title')) {
            titles.push([parent, parent.getAttribute('title')]);
            parent.removeAttribute('title');
        }
    }
    timer = setTimeout(() => show(node, text), delay);
}

root.addEventListener('pointerover', event => {
    if (event.pointerType === 'touch' || event.buttons) return hide();
    schedule(targetFor(event), 450);
}, {signal});
root.addEventListener('pointerout', event => {
    if (current && (!(event.relatedTarget instanceof Node) || !current.contains(event.relatedTarget))) hide();
}, {signal});
root.addEventListener('focusin', event => schedule(targetFor(event), 150), {signal});
root.addEventListener('focusout', hide, {signal});
root.addEventListener('pointerdown', hide, {capture: true, signal});
root.addEventListener('keydown', hide, {capture: true, signal});
document.addEventListener('scroll', hide, {capture: true, signal});
window.addEventListener('resize', hide, {signal});
window.addEventListener('blur', hide, {signal});
const observer = new MutationObserver(() => {
    if (!help.isConnected || !root.isConnected) cleanup();
    else if (current && !current.isConnected) hide();
});
observer.observe(document.body, {childList: true, subtree: true});
function cleanup() {
    hide();
    controller.abort();
    observer.disconnect();
    if (window.__nyatiControlHelpCleanup === cleanup) delete window.__nyatiControlHelpCleanup;
}
window.__nyatiControlHelpCleanup = cleanup;
