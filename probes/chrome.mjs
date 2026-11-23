import {spawn} from 'node:child_process';
import {mkdir, writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {once} from 'node:events';
import {setTimeout as delay} from 'node:timers/promises';

export async function stopChild(child, exit) {
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  if (await Promise.race([exit.then(() => true), delay(1500, false, {ref:false})])) return;
  child.kill('SIGKILL');
  await exit;
}

export async function startBrowser(binary, directory, port, timezone, locale) {
  const language = locale.split('-')[0];
  await mkdir(join(directory, 'Default'), {recursive:true});
  await writeFile(join(directory, 'Default', 'Preferences'), JSON.stringify({
    intl:{accept_languages:locale + ',' + language, selected_languages:locale + ',' + language}
  }));
  const child = spawn(binary, [
    '--headless=new', '--remote-debugging-pipe', '--no-first-run', '--no-default-browser-check',
    '--disable-background-networking', '--disable-component-update', '--disable-sync',
    '--force-webrtc-ip-handling-policy=disable_non_proxied_udp',
    '--user-data-dir=' + directory, '--proxy-server=http://127.0.0.1:' + port,
    '--proxy-bypass-list=<-loopback>', '--lang=' + locale, '--accept-lang=' + locale + ',' + language,
    'about:blank'
  ], {env:{...process.env, TZ:timezone}, stdio:['ignore','ignore','ignore','pipe','pipe']});
  await once(child, 'spawn');
  const exit = once(child, 'exit');
  const input = child.stdio[3], output = child.stdio[4];
  const pending = new Map();
  let sequence = 0, buffer = '';
  function disconnected() {
    for (const waiter of pending.values()) waiter.reject(new Error('CDP pipe closed'));
    pending.clear();
  }
  input.on('error', disconnected);
  output.on('error', disconnected);
  output.on('end', disconnected);
  output.setEncoding('utf8').on('data', chunk => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf('\0')) !== -1) {
      const message = JSON.parse(buffer.slice(0, end));
      buffer = buffer.slice(end + 1);
      const waiter = pending.get(message.id);
      if (!waiter) continue;
      pending.delete(message.id);
      if (message.error) waiter.reject(new Error(message.error.message));
      else waiter.resolve(message.result);
    }
  });
  function call(method, params = {}, sessionId) {
    const id = ++sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(method + ' timed out'));
      }, 12000);
      pending.set(id, {
        resolve:value => {clearTimeout(timer); resolve(value);},
        reject:error => {clearTimeout(timer); reject(error);}
      });
      input.write(JSON.stringify({id, method, params, sessionId}) + '\0');
    });
  }
  return {
    call,
    async evaluate(sessionId, expression, userGesture = false) {
      const response = await call('Runtime.evaluate', {expression, userGesture, awaitPromise:true, returnByValue:true}, sessionId);
      if (response.exceptionDetails) throw new Error(response.exceptionDetails.exception?.description ?? response.exceptionDetails.text);
      return response.result.value;
    },
    async disconnect() {
      input.end();
      output.destroy();
      return Promise.race([exit.then(() => true), delay(3000, false, {ref:false})]);
    },
    close:() => stopChild(child, exit)
  };
}
