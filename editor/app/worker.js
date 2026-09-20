// Debug snapshot worker: runs the language core (wasm) off the main thread.
//
// 取消语义：同步 Rust 求值无法合作式中断，宿主的时长预算是 terminate 整个
// Worker（wasm 实例随之销毁），下次分析再重建 Worker 并重新 init/fetch wasm。
import init, { analyze } from "../pkg/notist_editor.js";

const ready = init();

self.onmessage = async (event) => {
  const message = event.data;
  if (!message || message.type !== "analyze") return;
  try {
    await ready;
    const json = analyze(JSON.stringify(message.request));
    self.postMessage({
      type: "snapshot",
      request_id: message.request.request_id,
      json,
    });
  } catch (error) {
    self.postMessage({
      type: "failed",
      request_id: message.request ? message.request.request_id : null,
      message: String((error && error.message) || error),
    });
  }
};
