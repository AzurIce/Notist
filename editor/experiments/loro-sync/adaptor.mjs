import { CrdtType } from "loro-protocol";

// Compare causal vectors, never local notification revisions.
export function compareVersions(a, b) {
  let greater = false, less = false;
  for (const peer of new Set([...Object.keys(a.clocks), ...Object.keys(b.clocks)])) {
    const x = a.clocks[peer] || 0, y = b.clocks[peer] || 0;
    greater ||= x > y; less ||= x < y;
  }
  return greater && less ? undefined : greater ? 1 : less ? -1 : 0;
}

// Implements the official CrdtDocAdaptor interface. The only client document is
// our Rust/Wasm EditorDocument; this module never constructs an npm LoroDoc.
export class KernelAdaptor {
  crdtType = CrdtType.Loro;
  constructor(core, { onImport = () => {}, onError = () => {}, onJoin = () => {} } = {}) {
    this.core = core;
    this.identity = core.snapshot().version.identity;
    this.onImport = onImport; this.onError = onError; this.onJoin = onJoin;
    this.ready = false; this.destroyed = false; this.waiters = [];
    this.stop = core.subscribe(event => {
      if (!this.ready || this.destroyed || this.permission !== "write" || event.after.revision <= this.sentRevision) return;
      if (!["local", "undo", "redo"].includes(event.cause.kind)) return;
      const packet = core.exportUpdatesSince(this.sentVersion);
      this.ctx.send([Uint8Array.from(packet.data)]);
      this.sentVersion = core.snapshot().version;
      this.sentRevision = core.snapshot().revision;
    });
  }
  setCtx(ctx) { this.ctx = ctx; }
  getVersion() { return this.core.encodedVersion(); }
  cmpVersion(bytes) { return compareVersions(this.core.snapshot().version, this.core.decodeVersion(bytes)); }
  disconnected() { this.ready = false; this.serverVersion = undefined; }
  async handleJoinOk(response) {
    try {
      this.permission = response.permission;
      this.serverVersion = response.version.length ? this.core.decodeVersion(response.version) : { identity: this.identity, clocks: {} };
      const comparison = compareVersions(this.core.snapshot().version, this.serverVersion);
      this.ready = true;
      if (this.permission === "write" && comparison !== 0 && comparison !== -1) {
        const packet = response.version.length ? this.core.exportUpdatesSince(this.serverVersion) : this.core.exportSnapshot();
        this.ctx.send([Uint8Array.from(packet.data)]);
      }
      this.sentVersion = this.core.snapshot().version;
      this.sentRevision = this.core.snapshot().revision;
      this.checkReached();
      this.onJoin(response);
    } catch (error) { this.ready = false; this.onError(error); throw error; }
  }
  checkReached() {
    if (!this.serverVersion) return;
    const comparison = compareVersions(this.core.snapshot().version, this.serverVersion);
    if (comparison === 0 || comparison === 1) for (const waiter of this.waiters.splice(0)) waiter.resolve();
  }
  waitForReachingServerVersion() {
    return new Promise((resolve, reject) => { this.waiters.push({ resolve, reject }); this.checkReached(); });
  }
  applyUpdate(updates) {
    if (this.destroyed) return;
    try {
      for (const bytes of updates) {
        const result = this.core.importBinary(this.identity, bytes, "loro-websocket");
        this.onImport(bytes, result);
      }
      this.checkReached();
    } catch (error) { this.onError(error); throw error; }
  }
  onUpdateError(_updates, code, reason) {
    this.onError(new Error(`Server rejected update: ${code} ${reason || ""}`));
  }
  destroy() {
    if (this.destroyed) return;
    this.destroyed = true; this.ready = false; this.stop();
    for (const waiter of this.waiters.splice(0)) waiter.reject(new Error("Sync adapter disposed"));
  }
}
