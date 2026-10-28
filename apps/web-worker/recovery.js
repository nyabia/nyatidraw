import init, { RecoveryEngine, export_reduced_png, export_sprite_sheet } from "./ntdr.js";
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
    if (data.kind === "export-png") {
      const bytes = export_reduced_png(data.pixels, data.width, data.height, data.outputWidth, data.outputHeight);
      self.postMessage({ id, bytes }, [bytes.buffer]);
      return;
    }
    if (data.kind === "export-spritesheet") {
      const output = export_sprite_sheet(packet, data.columns);
      try {
        const png = output.take_png();
        const manifest = new TextEncoder().encode(output.take_manifest());
        self.postMessage({ id, files: [png, manifest] }, [png.buffer, manifest.buffer]);
      } finally { output.free(); }
      return;
    }
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
