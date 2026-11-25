import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { mkdtemp, rm, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawn } from "node:child_process";

const root = await mkdtemp(join(tmpdir(), "harbor-interaction-"));
const binary = process.env.HARBOR_TEST_OBSCURA;
assert.ok(binary, "Set HARBOR_TEST_OBSCURA to the pinned Obscura executable");
const helper = resolve(process.env.HARBOR_TEST_VIEWER || "dist/Profile Harbor.app/Contents/MacOS/profile-harbor-viewer");
const counts = new Map();
const proxy = http.createServer((req,res) => {
  const { pathname } = new URL(req.url);
  if (pathname.endsWith("/disconnect")) {
    proxy.close(); res.setHeader("Connection","close");
  }
  counts.set(pathname, (counts.get(pathname) || 0) + 1);
  res.setHeader("Content-Type", "text/html; charset=utf-8");
  res.end(pathname === "/trace" ? "<p>ip=9.9.9.9</p>" : `<!doctype html><html><head><title>Fixture</title><style>
  body{margin:0;height:3000px}input,button{position:absolute;left:10px;width:150px;height:40px}input{top:10px}button{top:70px}
  </style></head><body><input id="word"><button onclick="document.title='Clicked'">Press</button><p style="margin-top:150px">Fixture</p></body></html>`);
});
function run(cmd,args,input,timeout=60000) {
  return new Promise((ok,no)=>{
    const p=spawn(cmd,args,{stdio:["pipe","inherit","inherit"]});
    const timer=setTimeout(()=>p.kill("SIGKILL"),timeout);
    p.on("error",no);
    p.on("exit",(code,signal)=>{clearTimeout(timer);code===0?ok():no(Error(cmd+" failed: "+(signal||code)))});
    p.stdin.end(input);
  });
}
try {
  proxy.listen(0,"127.0.0.1");await once(proxy,"listening");
  const storage=join(root,"profile");await mkdir(storage);
  const config={name:"Interaction fixture",directory:storage, port:proxy.address().port,
    zone:"Asia/Tokyo", locale:"ja-JP", ip:"9.9.9.9",echo:"http://1.1.1.1/fixture",
    binary:resolve(binary),guardBinary:join(dirname(helper),"profile-harbor-guard")};
  const check=join(root,"ui-check");
  await run("xcrun",["swiftc","-O","-D","VIEWER_TEST","-parse-as-library","native/ObscuraConnection.swift",
    "native/ObscuraViewer.swift","tests/ObscuraInteraction.swift","-o",check],undefined,120000);
  await run(check,[],JSON.stringify(config));
  assert.ok(counts.get("/fixture")>=2 && counts.get("/fixture/next")>=1,"navigation must use the proxy");
} finally { proxy.closeAllConnections();proxy.close();await rm(root,{recursive:true,force:true}); }
