import { startFixture } from "../probes/fixtures.mjs";
import { spawn } from "node:child_process";
import { once } from "node:events";
import http from "node:http";

const fixtures = [];
let subscription;
const ipv6 = http.createServer((req, res) =>
  res.end("reachable direct IPv6 origin"),
);
ipv6.listen(0, "::1");
await once(ipv6, "listening");
try {
  for (const [i, country] of ["JP", "JP", "FR", "US", "GB"].entries()) {
    fixtures.push(
      await startFixture(
        "local-" + i,
        "ip=9.9.9." + (i + 1) + "\nloc=" + country,
      ),
    );
  }
  subscription = http.createServer((req, res) =>
    res.end(
      JSON.stringify({
        proxies: fixtures.map((f, i) => ({
          name: "Node " + i,
          type: "http",
          server: "127.0.0.1",
          port: f.port,
        })),
      }),
    ),
  );
  subscription.listen(0, "127.0.0.1");
  await once(subscription, "listening");
  const env = {
    ...process.env,
    HARBOR_TEST_SUB:
      "http://127.0.0.1:" + subscription.address().port + "/list",
    HARBOR_TEST_V6: String(ipv6.address().port),
    HARBOR_TEST_PORTS: fixtures.map((f) => f.port).join(","),
  };
  const child = spawn(
    "cargo",
    [
      "test",
      "--release",
      "--locked",
      "--manifest-path",
      "src-tauri/Cargo.toml",
      "desktop_flow",
      "--",
      "--ignored",
      "--nocapture",
    ],
    { env, stdio: "inherit" },
  );
  const [code] = await once(child, "exit");
  process.exitCode = code ?? 1;
} finally {
  subscription?.close();
  ipv6.close();
  await Promise.all(fixtures.map((f) => f.close()));
}
