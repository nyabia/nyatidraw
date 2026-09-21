import init, { RecoveryEngine } from "./ntdr.js";
import { storeWorkspace } from "./storage.js";

const ready = init().then(() => new RecoveryEngine());
let busy = false;
self.onmessage = async ({ data }) => {
  const { id, packet, download, timings } = data;
  if (busy) {
    self.postMessage({ id, error: "Recovery worker queue is full" });
    return;
  }
  busy = true;
  let engine;
  try {
    engine = await ready;
    const started = performance.now();
    const encodeStart = performance.timeOrigin + started;
    const bytes = engine.stage(packet);
    const encodeMs = performance.now() - started;
    await storeWorkspace(bytes);
    engine.accept();
    self.postMessage({ id, bytes: download ? bytes : null,
      metrics: timings ? { encodeMs, encodeStart, encodeEnd: encodeStart + encodeMs,
        totalMs: performance.now() - started, bytes: bytes.length } : null,
    }, download ? [bytes.buffer] : []);
  } catch (error) {
    engine?.abort();
    self.postMessage({ id, error: String(error) });
  } finally { busy = false; }
};
