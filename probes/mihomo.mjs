import {spawn} from 'node:child_process';
import http from 'node:http';
import net from 'node:net';
import {once} from 'node:events';
import {writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {stopChild} from './chrome.mjs';

async function reservePorts(count) {
  const servers = [];
  try {
    while (servers.length < count) {
      const server = net.createServer();
      server.listen(0, '127.0.0.1');
      await once(server, 'listening');
      servers.push(server);
    }
    return servers.map(server => server.address().port);
  } finally {
    await Promise.all(servers.map(server => new Promise(resolve => server.close(resolve))));
  }
}

export function request(options, data) {
  return new Promise((resolve, reject) => {
    const req = http.request({...options, agent:false, signal:AbortSignal.timeout(4000)}, res => {
      let body = '';
      res.setEncoding('utf8').on('data', chunk => body += chunk);
      res.once('error', reject);
      res.once('end', () => resolve({status:res.statusCode, body}));
    });
    req.once('error', reject);
    req.end(data);
  });
}

export async function startCore(binary, directory, fixtures) {
  const ports = await reservePorts(5);
  const socketPath = join(directory, 'control.sock');
  const config = {
    'external-controller-unix':socketPath, mode:'rule', 'log-level':'warning',
    'geo-auto-update':false, dns:{enable:false}, tun:{enable:false}, sniffer:{enable:false},
    profile:{'store-selected':false},
    'proxy-providers':{local:{type:'file', path:'./nodes.yaml'}},
    'proxy-groups':ports.map((_, i) => ({name:'SLOT_' + (i+1), type:'select', proxies:['REJECT'], use:['local']})),
    listeners:ports.map((port, i) => ({name:'slot-' + (i+1), type:'mixed', listen:'127.0.0.1', port, proxy:'SLOT_' + (i+1)})),
    rules:['MATCH,REJECT']
  };
  const proxies = fixtures.map(({name, port}) => ({type:'http', name, server:'127.0.0.1', port}));
  await writeFile(join(directory, 'nodes.yaml'), JSON.stringify({proxies}), {mode:0o600});
  const filename = join(directory, 'config.yaml');
  await writeFile(filename, JSON.stringify(config), {mode:0o600});
  const child = spawn(binary, ['-d', directory, '-f', filename], {stdio:['ignore','pipe','pipe']});
  await once(child, 'spawn');
  const exit = once(child, 'exit');
  const close = () => stopChild(child, exit);
  let log = '';
  const capture = chunk => {log = (log + chunk).slice(-3000);};
  child.stdout.on('data', capture);
  child.stderr.on('data', capture);
  async function api(path, method = 'GET', data) {
    const response = await request({socketPath, path, method, headers:{'content-type':'application/json'}}, JSON.stringify(data));
    if (response.status >= 300) throw new Error(path + ' HTTP ' + response.status + ' ' + response.body);
    return response.body ? JSON.parse(response.body) : null;
  }
  try {
    for (let i = 0; i < 80 && child.exitCode === null; i++) {
      try {
        const version = await api('/version');
        return {
          ports, version, api, close,
          select:(slot, name) => api('/proxies/SLOT_' + slot, 'PUT', {name}),
          async crash() {
            if (!child.kill('SIGKILL')) throw new Error('Mihomo was already stopped');
            const [, signal] = await exit;
            if (signal !== 'SIGKILL') throw new Error('Unexpected Mihomo exit');
          }
        };
      } catch { await delay(100); }
    }
    throw new Error('Mihomo startup failed: ' + log);
  } catch (error) {
    await close();
    throw error;
  }
}
