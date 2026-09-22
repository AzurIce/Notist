import { LoroWebsocketClient } from "loro-websocket";
import { KernelAdaptor } from "./adaptor.mjs";

// Upstream 0.6.2 calls async connect() from its constructor without handling the
// returned promise. Closing during initial connection otherwise creates an
// unhandled rejection. Keep the same rejected promise for explicit callers.
class HandledClient extends LoroWebsocketClient {
  connect(options) {
    const promise = super.connect(options);
    promise.catch(() => {});
    return promise;
  }
}

export function connectCore(core, { url, roomId, onState = () => {}, onImport, onError = () => {} }) {
  const identity = core.snapshot().version.identity;
  const adaptor = new KernelAdaptor(core, { onImport, onError });
  const client = new HandledClient({ url, onError,
    reconnect: { initialDelayMs: 100, maxDelayMs: 1500, jitter: 0.1 },
  });
  let joined = false, firstJoinStarted = false, destroyed = false;
  let resolveReady, rejectReady;
  const ready = new Promise((resolve, reject) => { resolveReady = resolve; rejectReady = reject; });
  const stop = client.onStatusChange(state => {
    if (state !== "connected") { joined = false; adaptor.disconnected(); }
    onState({ connection: state, joined });
    if (state === "connected" && !firstJoinStarted) {
      firstJoinStarted = true;
      client.join({ roomId, crdtAdaptor: adaptor,
        auth: new TextEncoder().encode(JSON.stringify(identity)),
        onStatusChange(state) { joined = state === "joined"; onState({ connection: client.getStatus(), joined, room: state }); },
      }).then(resolveReady, rejectReady);
    }
  });
  // Error is also available through ready for tests/callers. Always attach a
  // handler because an offline page need not await its first network join.
  ready.catch(error => { if (!destroyed) onError(error); });
  return {
    client, adaptor, ready,
    disconnect() { client.close(); },
    reconnect() { return client.connect(); },
    destroy() { destroyed = true; stop(); client.destroy(); adaptor.destroy(); rejectReady(new Error("Connection disposed")); },
  };
}
