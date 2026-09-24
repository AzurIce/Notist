const request = value => new Promise((resolve, reject) => { value.onsuccess = () => resolve(value.result); value.onerror = () => reject(value.error); });
const committed = transaction => new Promise((resolve, reject) => {
  transaction.oncomplete = resolve;
  transaction.onabort = transaction.onerror = () => reject(transaction.error || new Error("Storage transaction aborted"));
});

export class IndexedDbStore {
  durable = true;
  static async open(profile) {
    if (!navigator.locks) throw new Error("This browser needs Web Locks to protect a persistent node profile");
    let release;
    let acquired;
    const ready = new Promise((resolve, reject) => { acquired = { resolve, reject }; });
    const held = navigator.locks.request(`notist-editor-node:${profile}`, { ifAvailable: true }, async lock => {
      if (!lock) { acquired.reject(new Error("This node profile is already open in another tab")); return; }
      await new Promise(resolve => { release = resolve; acquired.resolve(); });
    });
    held.catch(acquired.reject);
    await ready;
    try {
      const opening = indexedDB.open("notist-editor-node", 1);
      opening.onupgradeneeded = () => { opening.result.createObjectStore("metadata"); opening.result.createObjectStore("journal"); };
      const database = await request(opening);
      const store = new IndexedDbStore(database, profile, () => { release(); return held; });
      const tx = database.transaction("metadata", "readwrite"); const done = committed(tx); const table = tx.objectStore("metadata");
      let id = await request(table.get([profile, "node-id"]));
      if (!id) { id = crypto.randomUUID(); table.put(id, [profile, "node-id"]); }
      await done; store.nodeId = id; return store;
    } catch (error) { release(); await held; throw error; }
  }
  constructor(database, profile, release) { this.database = database; this.profile = profile; this.release = release; }
  async load(document) {
    const tx = this.database.transaction("journal", "readonly"); const done = committed(tx);
    const entries = await request(tx.objectStore("journal").getAll(IDBKeyRange.bound([this.profile, document, 0], [this.profile, document, Number.MAX_SAFE_INTEGER])));
    await done; return entries;
  }
  async append(document, entry) {
    const tx = this.database.transaction(["journal", "metadata"], "readwrite", { durability: "strict" }); const done = committed(tx);
    try {
      const metadata = tx.objectStore("metadata"); const key = [this.profile, "head", document];
      const previous = await request(metadata.get(key)) || 0;
      if (entry.sequence !== previous + 1 || entry.packet.identity.document_id !== document) throw new Error("Journal sequence/identity mismatch");
      tx.objectStore("journal").add(entry, [this.profile, document, entry.sequence]);
      metadata.put(entry.sequence, key);
    } catch (error) { tx.abort(); await done.catch(() => {}); throw error; }
    await done;
  }
  async close() { this.database.close(); await this.release(); }
}

export class MemoryStore {
  durable = false;
  nodeId = crypto.randomUUID();
  records = new Map();
  async load(document) { return structuredClone(this.records.get(document) || []); }
  async append(document, entry) {
    const entries = this.records.get(document) || [];
    if (entry.sequence !== entries.length + 1) throw new Error("Journal sequence mismatch");
    entries.push(structuredClone(entry)); this.records.set(document, entries);
  }
  async close() {}
}
