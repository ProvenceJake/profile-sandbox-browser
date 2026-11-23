import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {once} from 'node:events';
import {access, mkdtemp, rm} from 'node:fs/promises';
import {constants} from 'node:fs';
import {join} from 'node:path';
import net from 'node:net';
import {setTimeout as delay} from 'node:timers/promises';
import {startFixture} from './fixtures.mjs';
import {startBrowser} from './chrome.mjs';
import {startCore, request} from './mihomo.mjs';

const chrome = process.env.CHROME_BIN || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const mihomo = process.env.MIHOMO_BIN || '/Applications/Clash Verge.app/Contents/MacOS/verge-mihomo';
await Promise.all([chrome, mihomo].map(path => access(path, constants.X_OK)));
const directory = await mkdtemp('/tmp/profile-m0-'); // Short path for the Unix socket.
const cleanup = [];
let interrupted = false;
for (const signal of ['SIGTERM', 'SIGINT']) {
  process.on(signal, () => {
    interrupted = true;
    process.exitCode = 1;
  });
}
function checkInterrupt() {
  if (interrupted) throw new Error('Probe interrupted');
}
async function eventually(check, label) {
  let last;
  for (let i = 0; i < 60; i++) {
    checkInterrupt();
    try { return await check(); }
    catch (error) { last = error; await delay(50); }
  }
  throw new Error(label + ': ' + last);
}
function listenToTunnel(port) {
  const socket = net.connect(port, '127.0.0.1');
  socket.on('error', () => {});
  cleanup.push(() => socket.destroy());
  socket.setTimeout(4000, () => socket.destroy(new Error('Tunnel timeout')));
  return socket;
}
async function tunnel(port) {
  const socket = listenToTunnel(port);
  await once(socket, 'connect');
  const response = once(socket, 'data');
  socket.write('CONNECT probe.invalid:443 HTTP/1.1\r\nHost: probe.invalid:443\r\n\r\n');
  assert.match((await response)[0].toString(), /^HTTP\/1\.[01] 200/);
  await ping(socket);
  return socket;
}
async function ping(socket) {
  const response = once(socket, 'data');
  socket.write('ping');
  assert.equal((await response)[0].toString(), 'ping');
}
async function inspectBrowser(browser, expectedNode, marker, origin) {
  const {targetId} = await browser.call('Target.createTarget', {url:'about:blank'});
  const {sessionId} = await browser.call('Target.attachToTarget', {targetId, flatten:true});
  const evaluate = (expression, gesture) => browser.evaluate(sessionId, expression, gesture);
  await browser.call('Page.navigate', {url:origin}, sessionId);
  await eventually(async () => assert.equal(await evaluate('typeof window.probe'), 'object'), 'loading fixture');
  const echo = await evaluate('fetch("/echo").then(r=>r.json())');
  assert.equal(echo.node, expectedNode);
  assert.equal(await evaluate('fetch("/cached").then(r=>r.text())'), expectedNode);
  assert.deepEqual(await evaluate('probe.markers()'), ['', null, null, null]);
  await evaluate('probe.markers(' + JSON.stringify(marker) + ')');
  const contexts = await evaluate('probe.readContexts()', true);
  return {evaluate, contexts, echo};
}
async function nodeAt(port) {
  const response = await request({host:'127.0.0.1', port, path:'http://localhost/echo', headers:{Host:'localhost'}});
  assert.equal(response.status, 200);
  return JSON.parse(response.body).node;
}

try {
  console.log('Chrome:', execFileSync(chrome, ['--version'], {encoding:'utf8'}).trim());
  const fixtures = [];
  for (let i = 1; i <= 5; i++) {
    const fixture = await startFixture('node-' + i);
    cleanup.push(fixture.close);
    fixtures.push(fixture);
  }
  const core = await startCore(mihomo, directory, fixtures);
  cleanup.push(core.close);
  console.log('Mihomo:', core.version.version);
  for (const [i, port] of core.ports.entries()) {
    assert.equal((await core.api('/proxies/SLOT_' + (i+1))).now, 'REJECT');
    await assert.rejects(nodeAt(port));
    await core.select(i+1, fixtures[i].name);
  }
  assert.deepEqual(await Promise.all(core.ports.map(nodeAt)), fixtures.map(f => f.name));
  console.log('PASS five default-deny slots and concurrent local exit markers');

  const old = await tunnel(core.ports[0]);
  const control = await tunnel(core.ports[1]);
  const connections = async () => (await core.api('/connections')).connections || [];
  const current = await eventually(async () => {
    const rows = await connections();
    for (const slot of ['slot-1','slot-2']) assert(rows.some(row => row.metadata.inboundName === slot));
    return rows;
  }, 'waiting for tagged connections');
  const closed = once(old, 'close');
  await core.select(1, 'node-3');
  for (const row of current.filter(row => row.metadata.inboundName === 'slot-1')) {
    await core.api('/connections/' + encodeURIComponent(row.id), 'DELETE');
  }
  await Promise.race([closed, delay(2000, null, {ref:false}).then(() => {throw new Error('Old tunnel survived switch');})]);
  await ping(control);
  assert.equal(await nodeAt(core.ports[0]), 'node-3');
  control.destroy();
  console.log('PASS switched slot 1, closed its old tunnel and kept slot 2 connected');

  await core.select(1, 'node-1');
  const profiles = [
    ['Asia/Tokyo', 'ja-JP', 'node-1'],
    ['Europe/Paris', 'fr-FR', 'node-2'],
    ['Europe/Berlin', 'de-DE', 'node-3'],
    ['America/Sao_Paulo', 'pt-BR', 'node-4'],
    ['America/New_York', 'en-US', 'node-5']
  ];
  const pages = [];
  const browsers = [];
  for (const [i, [zone, locale, node]] of profiles.entries()) {
    checkInterrupt();
    const browser = await startBrowser(chrome, join(directory, 'profile-' + i), core.ports[i], zone, locale);
    cleanup.push(browser.close);
    browsers.push(browser);
    const page = await inspectBrowser(browser, node, 'profile' + i, 'http://localhost:' + fixtures[4].port + '/');
    pages.push(page);
    console.log('OBSERVED', locale, JSON.stringify({contexts:page.contexts, header:page.echo.acceptLanguage}));
    for (const scope of Object.values(page.contexts)) {
      assert.equal(scope.zone, zone);
      assert.equal(scope.language, locale);
      assert.deepEqual(scope.languages, [locale, locale.split('-')[0]]);
    }
    assert.equal(page.echo.acceptLanguage.split(',')[0], locale);
  }
  for (const [i, page] of pages.entries()) {
    const marker = 'profile' + i;
    assert.equal(await page.evaluate('fetch("/cached",{cache:"only-if-cached",mode:"same-origin"}).then(r=>r.text())'), profiles[i][2]);
    assert.deepEqual(await page.evaluate('probe.markers()'), ['marker=' + marker, marker, marker, marker]);
    assert.equal(await page.evaluate('fetch("/echo").then(r=>r.json()).then(x=>x.node)'), profiles[i][2]);
  }
  console.log('PASS five browser profiles, independent cookies/localStorage/IndexedDB/CacheStorage and six locale/timezone contexts');
  await core.select(1, 'REJECT');
  await assert.rejects(nodeAt(core.ports[0]));
  assert.equal(await nodeAt(core.ports[1]), 'node-2');
  await core.crash();
  const direct = await request({host:'127.0.0.1', port:fixtures[4].port, path:'/echo'});
  assert.equal(JSON.parse(direct.body).node, 'node-5');
  for (const page of pages) {
    const blocked = await page.evaluate('fetch("/echo",{cache:"no-store", signal:AbortSignal.timeout(3000)}).then(()=>false,()=>true)');
    assert.equal(blocked, true);
  }
  console.log('PASS browser fixture requests failed after SIGKILL, with a reachable direct origin');
  for (const browser of browsers) assert.equal(await browser.disconnect(), true, 'Chrome outlived CDP pipe by 3s');
  console.log('PASS all five browser processes exited within 3s of closing their CDP pipes');
  checkInterrupt();
} finally {
  try {
    for (const close of cleanup.reverse()) await close();
  } finally { await rm(directory, {recursive:true, force:true}); }
}
