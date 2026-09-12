// Focus stays in chrome overlays. No drawing samples or editor commands here.
if (!overlay) return;
const previous = document.activeElement;
const controls = () => [...overlay.querySelectorAll('button:not(:disabled),input:not(:disabled),select:not(:disabled),[tabindex="0"]')]
    .filter(node => node.getClientRects().length);
const first = controls()[0];
first?.focus();
function focusInside(event) {
    if (overlay.isConnected && !overlay.contains(event.target)) controls()[0]?.focus();
}
function tabInside(event) {
    if (event.key !== 'Tab') return;
    const items = controls();
    if (!items.length) return;
    const index = items.indexOf(document.activeElement);
    if (event.shiftKey ? index <= 0 : index < 0 || index === items.length - 1) {
        event.preventDefault();
        (event.shiftKey ? items.at(-1) : items[0]).focus();
    }
}
document.addEventListener('focusin', focusInside);
overlay.addEventListener('keydown', tabInside);
const observer = new MutationObserver(() => {
    if (overlay.isConnected) return;
    document.removeEventListener('focusin', focusInside);
    overlay.removeEventListener('keydown', tabInside);
    observer.disconnect();
    if (previous?.isConnected) previous.focus();
});
observer.observe(document.body, { childList: true, subtree: true });
