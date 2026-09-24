// Host adapter for the Rust/Wasm kernel. It contains no editor or language API.
// Subscribers run in a microtask after the Wasm call AND the caller's frontend
// transaction return; they never run inside a borrowed Wasm Document.
function immutable(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    for (const child of Object.values(value)) immutable(child);
    Object.freeze(value);
  }
  return value;
}

export class CoreError extends Error {
  constructor(value) {
    super(value.message || value.code || String(value));
    this.name = "CoreError";
    Object.assign(this, value);
  }
}

function normalizeError(error) {
  if (typeof error !== "string") return error;
  let value;
  try { value = JSON.parse(error); } catch { value = { code: "core_error", message: error }; }
  return new CoreError(value);
}

export class EditorDocument {
  #native;
  #listeners = new Set();
  #pending = [];
  #scheduled = false;
  #notifying = false;
  #disposed = false;
  #onListenerError;

  static create(Binding, { identity, writer, text = "", onListenerError } = {}) {
    try { return new EditorDocument(new Binding(JSON.stringify(identity), writer, text), onListenerError); }
    catch (error) { throw normalizeError(error); }
  }

  static restore(Binding, packet, { writer, onListenerError } = {}) {
    try { return new EditorDocument(Binding.from_snapshot(JSON.stringify(packet), writer), onListenerError); }
    catch (error) { throw normalizeError(error); }
  }

  constructor(native, onListenerError = error => console.error("Editor core subscriber failed", error)) {
    this.#native = native;
    this.#onListenerError = onListenerError;
  }

  #call(method, ...args) {
    if (this.#disposed) throw new Error("EditorDocument has been disposed");
    try { return this.#native[method](...args); }
    catch (error) { throw normalizeError(error); }
  }

  #mutate(method, ...args) {
    if (this.#notifying) throw new Error("Schedule observer-initiated edits after notification delivery");
    const value = this.#call(method, ...args);
    this.flushEvents();
    return typeof value === "string" ? immutable(JSON.parse(value)) : value;
  }

  // The node may import through a shared Rust handle rather than this wrapper.
  flushEvents() {
    this.#pending.push(...JSON.parse(this.#call("take_events")).map(immutable));
    if (!this.#scheduled && this.#pending.length) {
      this.#scheduled = true;
      queueMicrotask(() => {
        this.#scheduled = false;
        if (this.#disposed) return;
        const events = this.#pending.splice(0);
        this.#notifying = true;
        try {
          for (const event of events) {
            for (const listener of [...this.#listeners]) {
              if (!this.#listeners.has(listener) || event.after.revision <= listener.fromRevision) continue;
              try { listener.callback(event); }
              catch (error) {
                // A subscriber failure does not turn an already-committed write
                // into an apparent failed write that callers might retry.
                try { this.#onListenerError(error); } catch (reportError) { console.error(reportError); }
              }
            }
          }
        } finally { this.#notifying = false; }
      });
    }
  }

  snapshot() { return immutable(JSON.parse(this.#call("snapshot"))); }
  encodedVersion() { return this.#call("encoded_version"); }
  decodeVersion(bytes) { return immutable(JSON.parse(this.#call("decode_version", bytes))); }
  importBinary(identity, bytes, origin = "remote") { return this.#mutate("import_binary", JSON.stringify(identity), bytes, origin); }
  get writerId() { return this.#call("writer_id"); }
  get undoState() { return immutable(JSON.parse(this.#call("undo_state"))); }

  transact({ expectedVersion, edits, origin = "host", undoMetadata = null, undoPositions = [] }) {
    return this.#mutate("transact", JSON.stringify({ expected_version: expectedVersion, edits, origin, undo_metadata: undoMetadata, undo_positions: undoPositions }));
  }
  undo(metadata = null, positions = []) { return this.#mutate("undo", JSON.stringify({ metadata, positions })); }
  redo(metadata = null, positions = []) { return this.#mutate("redo", JSON.stringify({ metadata, positions })); }
  beginUndoGroup() { return this.#mutate("begin_undo_group"); }
  endUndoGroup() { return this.#mutate("end_undo_group"); }
  clearUndo() { return this.#mutate("clear_undo"); }
  exportSnapshot() { return JSON.parse(this.#call("export_snapshot")); }
  exportUpdatesSince(version) { return JSON.parse(this.#call("export_updates_since", JSON.stringify(version))); }
  import(packet, origin = "remote") { return this.#mutate("import_updates", JSON.stringify(packet), origin); }
  anchorAt(offset, affinity = "after") { return immutable(JSON.parse(this.#call("anchor_at", offset, JSON.stringify(affinity)))); }
  resolveAnchor(anchor) { return immutable(JSON.parse(this.#call("resolve_anchor", JSON.stringify(anchor)))); }

  subscribe(callback) {
    const listener = { callback, fromRevision: this.snapshot().revision };
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  dispose() {
    if (this.#disposed) return;
    if (this.#notifying) throw new Error("Dispose after notification delivery");
    this.#listeners.clear(); this.#pending.length = 0;
    this.#native.free(); this.#disposed = true;
  }
}
