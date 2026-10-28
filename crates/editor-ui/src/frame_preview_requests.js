window.__nyatidrawFramePreviewDispose?.();
const list = document.querySelector('.animation-frames');
if (list) {
    const listeners = new AbortController();
    let scheduled = 0;
    let previous = '';
    let disposed = false;

    function requestVisible() {
        scheduled = 0;
        if (disposed || !list.isConnected) return;
        const bounds = list.getBoundingClientRect();
        const buttons = [...list.querySelectorAll('[data-frame-id]')];
        const visible = [];
        if (bounds.width > 0 && bounds.height > 0) {
            buttons.forEach((button, index) => {
                const rect = button.getBoundingClientRect();
                if (rect.right > bounds.left && rect.left < bounds.right) visible.push(index);
            });
        }
        const requested = visible.slice(0, 24);
        if (visible.length) {
            for (let distance = 1; distance <= 2 && requested.length < 24; distance++) {
                const before = visible[0] - distance;
                const after = visible[visible.length - 1] + distance;
                if (before >= 0) requested.push(before);
                if (after < buttons.length && requested.length < 24) requested.push(after);
            }
        }
        const message = [list.dataset.framePreviewEpoch,
            requested.map(index => buttons[index].dataset.frameId)];
        const key = JSON.stringify(message);
        if (key !== previous) {
            previous = key;
            dioxus.send(message);
        }
    }

    function schedule() {
        if (!disposed && !scheduled) scheduled = requestAnimationFrame(requestVisible);
    }

    function dispose() {
        disposed = true;
        listeners.abort();
        cancelAnimationFrame(scheduled);
        resize.disconnect();
        mutations.disconnect();
        if (window.__nyatidrawFramePreviewDispose === dispose) {
            delete window.__nyatidrawFramePreviewDispose;
        }
    }

    const resize = new ResizeObserver(schedule);
    resize.observe(list);
    const mutations = new MutationObserver(records => {
        if (!list.isConnected) return dispose();
        if (records.some(record => record.target === list || list.contains(record.target))) schedule();
    });
    mutations.observe(document.documentElement, {
        childList: true, subtree: true, attributes: true,
        attributeFilter: ['data-frame-id', 'data-frame-preview-epoch']
    });
    list.addEventListener('scroll', schedule, { passive: true, signal: listeners.signal });
    window.__nyatidrawFramePreviewDispose = dispose;
    schedule();
}
