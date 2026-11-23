(async () => {
  function readEnvironment() {
    const date = Intl.DateTimeFormat().resolvedOptions();
    return {
      zone: date.timeZone,
      language: navigator.language,
      dateLocale: date.locale,
      numberLocale: Intl.NumberFormat().resolvedOptions().locale,
    };
  }
  const source = 'postMessage((' + readEnvironment.toString() + ')())';
  const url = URL.createObjectURL(new Blob([source], {type: 'text/javascript'}));
  let worker;
  try {
    worker = new Worker(url);
    const workerEnvironment = await new Promise((resolve, reject) => {
      worker.onmessage = event => resolve(event.data);
      worker.onerror = reject;
    });
    return [readEnvironment(), workerEnvironment];
  } finally {
    worker?.terminate();
    URL.revokeObjectURL(url);
  }
})()
