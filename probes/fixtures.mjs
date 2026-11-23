import {readFile} from 'node:fs/promises';
import http from 'node:http';
import {once} from 'node:events';

const environment = '({zone:Intl.DateTimeFormat().resolvedOptions().timeZone,dateLocale:Intl.DateTimeFormat().resolvedOptions().locale,numberLocale:Intl.NumberFormat().resolvedOptions().locale,language:navigator.language,languages:navigator.languages})';
const fixtureScript = await readFile(new URL('../tests/fixtures/page.js', import.meta.url), 'utf8');

export async function startFixture(name, trace = 'ip=192.0.2.1\nloc=JP') {
  const sockets = new Set();
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, 'http://localhost');
    if (url.hostname !== 'localhost' && !url.hostname.endsWith('.harbor-probe.invalid')) {
      response.writeHead(403).end();
      return;
    }
    const files = {
      '/': ['text/html', '<!doctype html><meta charset="utf-8"><title>Profile probe</title><script src="/page.js" defer></script>'],
      '/format-frame': ['text/html', '<!doctype html><script>parent.postMessage(' + environment + ', "*")</script>'],
      '/page.js': ['text/javascript', fixtureScript],
      '/trace': ['text/plain',trace],
      '/cached': ['text/plain', name],
      '/echo': ['application/json', JSON.stringify({node:name, acceptLanguage:request.headers['accept-language']})],
      '/worker.js': ['text/javascript', 'postMessage(' + environment + ')'],
      '/shared.js': ['text/javascript', 'onconnect=e=>e.ports[0].postMessage(' + environment + ')'],
      '/service.js': ['text/javascript', 'oninstall=e=>e.waitUntil(self.skipWaiting()); onactivate=e=>e.waitUntil(clients.claim()); onmessage=e=>e.ports[0].postMessage(' + environment + ')'],
    };
    const file = files[url.pathname];
    if (!file) {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, {'content-type':file[0], 'cache-control':url.pathname === '/cached' ? 'max-age=86400' : 'no-store'}).end(file[1]);
  });
  // A local echo tunnel lets the probe test a live connection across a node switch.
  server.on('connect', (request, socket, head) => {
    const isHttp = /^(localhost|127\.0\.0\.1|[a-z0-9-]+\.harbor-probe\.invalid):\d+$/.test(request.url);
    if (!isHttp && request.url !== 'probe.invalid:443') return socket.destroy();
    socket.write('HTTP/1.1 200 Connection Established\r\n\r\n');
    if (isHttp) {
      if (head.length) socket.unshift(head);
      server.emit('connection', socket);
    } else {
      if (head.length) socket.write(head);
      socket.pipe(socket);
    }
  });
  server.on('connection', socket => {
    sockets.add(socket);
    socket.on('error', () => {});
    socket.on('close', () => sockets.delete(socket));
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return {name, port:server.address().port, async close() {
    for (const socket of sockets) socket.destroy();
    await new Promise(resolve => server.close(resolve));
  }};
}
