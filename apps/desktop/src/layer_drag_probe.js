// Runs only for the marked scratch acceptance project. Real DOM and Dioxus
// handlers are exercised; these synthetic browser events are not physical drag.
try {
    const mode = '__PROBE_MODE__';
    const plans = {
        inside: ['r-1', 'g-11', 'inside', '11', '19'],
        below: ['r-1', 'r-2', 'below', '11', '0'],
        above: ['r-1', 'r-20', 'above', '11', '19'],
        group: ['g-11', 'g-10', 'below', '100', '0'],
        cancel: ['r-1', 'g-10', 'inside', null, null],
        stale: ['r-1', 'g-10', 'inside', '11', '18'],
    };
    const [sourceKey, targetKey, position, parent, index] = plans[mode];
    const row = key => document.querySelector(`[data-layer-key="${key}"]`);
    const until = async predicate => {
        const deadline = performance.now() + 10000;
        while (performance.now() < deadline) {
            const result = predicate();
            if (result) return result;
            await new Promise(resolve => setTimeout(resolve, 20));
        }
        throw new Error('drag DOM condition timed out');
    };
    await until(() => row(sourceKey) && row(targetKey) && row('r-20'));
    const source = row(sourceKey).querySelector('.layer-thumb');
    const transfer = new DataTransfer();
    source.dispatchEvent(new DragEvent('dragstart', {bubbles:true, cancelable:true, dataTransfer:transfer}));
    await until(() => document.querySelector('.layer-drop-targets'));
    if (mode === 'cancel') {
        source.dispatchEvent(new DragEvent('dragend', {bubbles:true, dataTransfer:transfer}));
        await until(() => !document.querySelector('.layer-drop-targets'));
    } else if (mode === 'stale') {
        // Change authoritative order with the real toolbar while the drag
        // still holds its old revision. Drop must not apply its obsolete index.
        const down = document.querySelector('.layer-actions button[title="선택 레이어를 한 칸 아래로"]');
        if (!down || down.disabled) throw new Error('layer move button unavailable');
        down.click();
        await until(() => row(sourceKey).dataset.index === index);
        const zone = row(targetKey).querySelector(`.layer-drop-zone.${position}`);
        zone.dispatchEvent(new DragEvent('drop', {bubbles:true, cancelable:true, dataTransfer:transfer}));
        await until(() => !document.querySelector('.layer-drop-targets'));
        await until(() => document.querySelector('.layers-panel .command-error')?.textContent.includes('드래그 중 문서가 변경'));
        if (row(sourceKey).dataset.parent !== parent || row(sourceKey).dataset.index !== index) throw new Error('stale drop moved artwork');
    } else {
        const zone = await until(() => row(targetKey).querySelector(`.layer-drop-zone.${position}`));
        const rect = zone.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) throw new Error('drop zone has no hit area');
        zone.dispatchEvent(new DragEvent('dragover', {bubbles:true, cancelable:true, dataTransfer:transfer}));
        await until(() => zone.classList.contains('active'));
        const marker = getComputedStyle(zone, position === 'inside' ? null : '::after');
        if (position === 'inside' ? parseFloat(marker.outlineWidth) < 2 : parseFloat(marker.height) < 2) {
            throw new Error('insertion marker is missing');
        }
        zone.dispatchEvent(new DragEvent('drop', {bubbles:true, cancelable:true, dataTransfer:transfer}));
        source.dispatchEvent(new DragEvent('dragend', {bubbles:true, dataTransfer:transfer}));
        await until(() => row(sourceKey).dataset.parent === parent && row(sourceKey).dataset.index === index);
        await until(() => !document.querySelector('.layer-drop-targets'));
    }
    dioxus.send('passed');
} catch (error) {
    dioxus.send(`failed:${error.message}`);
}
