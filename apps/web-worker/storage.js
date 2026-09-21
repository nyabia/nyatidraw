function database() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("nyatidraw-web-v1", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("workspace");
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
    request.onblocked = () => reject(new Error("다른 탭에서 저장소를 사용 중입니다. 해당 탭을 닫아주세요."));
  });
}
export async function loadWorkspace() {
  const db = await database();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction("workspace", "readonly");
      const request = tx.objectStore("workspace").get("current");
      request.onsuccess = () => resolve(request.result || null);
      request.onerror = () => reject(request.error);
    });
  } finally { db.close(); }
}
export async function storeWorkspace(bytes) {
  const db = await database();
  try {
    await new Promise((resolve, reject) => {
      const tx = db.transaction("workspace", "readwrite");
      const store = tx.objectStore("workspace");
      const previous = store.get("current");
      previous.onsuccess = () => {
        const oldBytes = previous.result;
        if (oldBytes && String.fromCharCode(...oldBytes.slice(0, 8)) === "NYWEB001") {
          store.put(oldBytes, "legacy-before-ntdr");
        }
        store.put(bytes, "current");
      };
      tx.oncomplete = resolve;
      tx.onabort = () => reject(tx.error || new Error("브라우저 저장이 취소되었습니다."));
      tx.onerror = () => reject(tx.error);
    });
  } finally { db.close(); }
}
