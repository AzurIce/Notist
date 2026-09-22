// A separate key for each demonstration replica lets two frames share an
// origin without overwriting one another's offline history.
export async function openStore(key) {
  let release = () => {};
  if (navigator.locks) await new Promise((resolve, reject) => {
    navigator.locks.request(`notist-sync/${key}`, { ifAvailable: true }, async lock => {
      if (!lock) { reject(new Error("这个实验副本已在其他页面打开，请使用另一个副本链接。")); return; }
      const held = new Promise(done => { release = done; });
      resolve(); await held;
    }).catch(reject);
  });
  const db = await new Promise((resolve, reject) => {
    const request = indexedDB.open("notist-loro-sync-experiment", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("replicas");
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => { release(); reject(request.error); };
  });
  return {
    read() {
      return new Promise((resolve, reject) => {
        const request = db.transaction("replicas").objectStore("replicas").get(key);
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });
    },
    write(value) {
      return new Promise((resolve, reject) => {
        const transaction = db.transaction("replicas", "readwrite");
        transaction.objectStore("replicas").put(value, key);
        transaction.oncomplete = resolve;
        transaction.onerror = () => reject(transaction.error);
        transaction.onabort = () => reject(transaction.error || new Error("Local save aborted"));
      });
    },
    close() { db.close(); release(); },
  };
}
