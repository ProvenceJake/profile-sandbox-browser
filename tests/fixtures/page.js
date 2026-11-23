function getEnvironment(context = window) {
  const {navigator, Intl} = context;
  return {
    zone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    dateLocale: Intl.DateTimeFormat().resolvedOptions().locale,
    numberLocale: Intl.NumberFormat().resolvedOptions().locale,
    language: navigator.language,
    languages: [...navigator.languages]
  };
}

async function readContexts() {
  const popup = window.open('about:blank');
  if (!popup) throw new Error('Fixture popup did not open');
  const frame = document.createElement('iframe');
  const loaded = new Promise(resolve => frame.onload = resolve);
  document.body.append(frame);
  await loaded;
  const result = {
    page: getEnvironment(),
    frame: getEnvironment(frame.contentWindow),
    popup: getEnvironment(popup)
  };
  popup.close();
  frame.remove();
  const worker = new Worker('/worker.js');
  try {
    result.worker = await new Promise(resolve => worker.onmessage = event => resolve(event.data));
  } finally { worker.terminate(); }
  const shared = new SharedWorker('/shared.js');
  try {
    result.shared = await new Promise(resolve => shared.port.onmessage = event => resolve(event.data));
  } finally { shared.port.close(); }
  await navigator.serviceWorker.register('/service.js');
  const registration = await navigator.serviceWorker.ready;
  const channel = new MessageChannel();
  const reply = new Promise(resolve => channel.port1.onmessage = event => resolve(event.data));
  registration.active.postMessage('read', [channel.port2]);
  result.service = await reply;
  channel.port1.close();
  return result;
}

async function readFormatContexts() {
  const result = await readContexts();
  const frame = document.createElement('iframe');
  const origin = 'http://format.harbor-probe.invalid';
  try {
    result.crossSiteFrame = await new Promise(resolve => {
      function receive(event) {
        if (event.origin !== origin || event.source !== frame.contentWindow) return;
        window.removeEventListener('message', receive);
        resolve(event.data);
      }
      window.addEventListener('message', receive);
      frame.src = origin + '/format-frame';
      document.body.append(frame);
    });
  } finally { frame.remove(); }
  return result;
}

async function markers(writeValue) {
  const db = await new Promise((resolve, reject) => {
    const open = indexedDB.open('probe', 1);
    open.onupgradeneeded = () => open.result.createObjectStore('values');
    open.onerror = () => reject(open.error);
    open.onsuccess = () => resolve(open.result);
  });
  const cache = await caches.open('probe');
  try {
    if (writeValue !== undefined) {
      const tx = db.transaction('values', 'readwrite');
      tx.objectStore('values').put(writeValue, 'marker');
      await new Promise((resolve, reject) => {
        tx.oncomplete = resolve;
        tx.onabort = () => reject(tx.error);
      });
      await cache.put('/marker', new Response(writeValue));
      localStorage.setItem('marker', writeValue);
      document.cookie = 'marker=' + writeValue;
    }
    const indexed = await new Promise((resolve, reject) => {
      const read = db.transaction('values').objectStore('values').get('marker');
      read.onsuccess = () => resolve(read.result ?? null);
      read.onerror = () => reject(read.error);
    });
    const cached = await cache.match('/marker');
    return [document.cookie, localStorage.getItem('marker'), indexed, cached ? await cached.text() : null];
  } finally { db.close(); }
}

window.probe = {readContexts, readFormatContexts, markers};
