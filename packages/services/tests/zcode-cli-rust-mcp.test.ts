import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { createServer, request as httpRequest } from "node:http";
import { once } from "node:events";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { z } from "zod";
import { event, end, fixture, waitForFile } from "./zcode-cli-rust-fixture.js";
import { httpServer, listMcp, stdioServer } from "./zcode-cli-rust-mcp-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

function respond(request: any, response: any) {
  response.writeHead(200, { "Content-Type": "text/event-stream" });
  if (request.messages.at(-1).role !== "tool") {
    const name = request.tools.find((t: any) => t.function.name.startsWith("mcp__"))?.function.name;
    event(response, {
      tool_calls: [
        {
          index: 0,
          id: "mcp-call",
          type: "function",
          function: {
            name: name ?? "missing_mcp",
            arguments: JSON.stringify({ text: "echo input" }),
          },
        },
      ],
    });
    end(response, "tool_calls");
  } else {
    event(response, { content: "done" });
    end(response, "stop");
  }
}

test("MCP stdio discovery and execution use session isolation, explicit workspace reuse and status-only observation", async () => {
  const f = await fixture({ mode: "yolo", respond });
  try {
    const server = await stdioServer(f.root);
    const h = f.start();
    const statuses = await listMcp(h, f.cwd, [server.config]);
    assert.equal(statuses.statuses[server.config.name]?.status, "connected");
    const before = await readFile(server.started, "utf8");
    assert.equal(
      (await listMcp(h, f.cwd, [], "status")).statuses[server.config.name]?.toolCount,
      1,
    );
    assert.equal(await readFile(server.started, "utf8"), before);
    const outputs: any[] = [];
    for (const isolation of ["session", "session", "workspace", "workspace"]) {
      const ack = await h.command(
        h.envelope("createSession", null, {
          workspaceId: f.cwd,
          mcpServers: [{ ...server.config, isolation }],
        }),
      );
      const sid = (ack.result as { sessionId: string }).sessionId;
      await h.subscribe(`conversation/${sid}`);
      await h.command(h.envelope("sendText", sid, { text: "call MCP" }));
      await h.completed(sid);
      outputs.push(JSON.parse(f.requests.at(-1)!.messages.at(-1).content));
    }
    assert.notEqual(outputs[0].pid, outputs[1].pid);
    assert.equal(outputs[2].pid, outputs[3].pid);
    assert.equal(outputs[0].probe, "scoped");
    assert.equal(
      f.requests[0]!.tools.find((t: any) => t.function.name.startsWith("mcp__")).function.name,
      "mcp__local_echo__echo",
    );
    assert.deepEqual((await listMcp(h, f.cwd, [])).statuses, {});
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
    for (const pid of (await readFile(server.started, "utf8")).trim().split("\n").map(Number))
      assert.throws(() => process.kill(pid, 0));
  } finally {
    await f.close();
  }
});

for (const transport of ["http", "sse"] as const)
  test(`MCP ${transport} uses configured headers and carries tool results into the next model request`, async () => {
    const f = await fixture({ mode: "yolo", respond });
    const server = await httpServer(transport);
    try {
      const h = f.start();
      const ack = await h.command(
        h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
      );
      const sid = (ack.result as { sessionId: string }).sessionId;
      await h.subscribe(`conversation/${sid}`);
      await h.command(h.envelope("sendText", sid, { text: "MCP HTTP" }));
      await h.completed(sid);
      assert.match(f.requests[1]!.messages.at(-1).content, /HTTP echo input/);
      assert.match(f.requests[1]!.messages.at(-1).content, /echoed/);
      assert.ok(server.calls.every((c) => c.headers["x-fixture"] === "configured"));
      assert.equal(server.calls.filter((c) => c.method === "tools/call").length, 1);
      assert.deepEqual(h.schemaErrors, []);
      await h.close();
    } finally {
      try {
        await f.close();
      } finally {
        await server.close();
      }
    }
  });
// MCP HTTP 传输必须走与模型/WebFetch 同一套代理解析（docs/specs/rust-net-proxy.md）。
// 本地 forward 代理转发所有请求并统计 MCP 路径：两侧的 MCP 握手都必须经它过去。
// 模型 fixture 仍在 127.0.0.1，由 ZCODE_NO_PROXY 显式绕过，所以这里只观察 MCP 流量。
async function mcpProxyRoutes(
  configure: (proxyUrl: string, cwd: string) => Promise<Record<string, string>>,
): Promise<string[]> {
  const server = await httpServer("http");
  const mcpConfig = { ...server.config, url: server.config.url.replace("127.0.0.1", "localhost") };
  let mcpRequests = 0;
  const proxy = createServer((req, res) => {
    const target = new URL(req.url ?? "/", "http://127.0.0.1");
    if (target.pathname.includes("/http")) mcpRequests += 1;
    const upstream = httpRequest(
      {
        hostname: target.hostname,
        port: target.port,
        path: `${target.pathname}${target.search}`,
        method: req.method,
        headers: req.headers,
      },
      (reply) => {
        res.writeHead(reply.statusCode ?? 502, reply.headers);
        reply.pipe(res);
      },
    );
    upstream.on("error", () => res.destroy());
    req.pipe(upstream);
  });
  proxy.listen(0, "127.0.0.1");
  await once(proxy, "listening");
  const proxyAddress = proxy.address();
  assert(proxyAddress && typeof proxyAddress !== "string");
  const proxyUrl = `http://127.0.0.1:${proxyAddress.port}`;
  const observed: string[] = [];
  try {
    for (const kind of ["node", "rust"] as const) {
      const root = await mkdtemp(join(tmpdir(), `zcode-mcp-proxy-${kind}-`));
      const env = await configure(proxyUrl, join(root, "workspace"));
      const f = await fixture(
        kind === "node"
          ? {
              root,
              mode: "yolo",
              respond,
              command: process.execPath,
              args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
              env,
            }
          : { root, mode: "yolo", respond, env },
      );
      try {
        const before = mcpRequests;
        const h = f.start();
        // createSession 即触发 MCP 连接；握手经代理过去就算这条传输走了代理。
        const ack = await h.command(
          h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [mcpConfig] }),
        );
        const sid = (ack.result as { sessionId: string }).sessionId;
        assert.ok(sid);
        // TS 在 createSession 就连 MCP；Rust 在首轮装配工具面时才连——两侧都发一轮再等代理计数。
        await h.subscribe(`conversation/${sid}`);
        await h.command(h.envelope("sendText", sid, { text: "MCP proxied" }));
        const deadline = Date.now() + 15000;
        while (mcpRequests === before && Date.now() < deadline)
          await new Promise((r) => setTimeout(r, 100));
        observed.push(`${kind}:${mcpRequests > before ? "proxied" : "direct"}`);
      } finally {
        await f.close();
      }
    }
    return observed;
  } finally {
    proxy.closeAllConnections();
    await new Promise<void>((r) => proxy.close(() => r()));
    await server.close();
  }
}

test("MCP HTTP transport goes through the configured proxy in both runtimes", async () => {
  const observed = await mcpProxyRoutes(async (proxyUrl) => ({
    ZCODE_HTTP_PROXY: proxyUrl,
    ZCODE_NO_PROXY: "127.0.0.1",
  }));
  assert.deepEqual(observed, ["node:proxied", "rust:proxied"]);
});

// docs/specs/rust-net-proxy.md「配置文件 `network` 段」：不设任何代理环境变量，只在配置文件里写 network。
// Node app-server 的 MCP 连接池与模型 registry 在进程启动时 createConfig({ env })，只读用户层；
// Rust 原先连用户层也忽略而直连。用户层 → 两侧 MCP 都经代理；项目层 → 两侧 MCP 都直连。
const writeNetwork = async (dir: string, proxyUrl: string) => {
  await mkdir(dir, { recursive: true });
  await writeFile(
    join(dir, "config.json"),
    JSON.stringify({ network: { httpProxy: proxyUrl, noProxy: "127.0.0.1" } }),
  );
  return {};
};

test("user config network section routes MCP through the proxy in both runtimes", async () => {
  const observed = await mcpProxyRoutes((proxyUrl, cwd) =>
    writeNetwork(join(dirname(cwd), ".zcode", "cli"), proxyUrl),
  );
  assert.deepEqual(observed, ["node:proxied", "rust:proxied"]);
});

test("project config network section does not reach the process-level MCP pool in either runtime", async () => {
  const observed = await mcpProxyRoutes((proxyUrl, cwd) =>
    writeNetwork(join(cwd, ".zcode"), proxyUrl),
  );
  assert.deepEqual(observed, ["node:direct", "rust:direct"]);
});

// 工具子进程环境读工作区合并视图的 network（TS executionPort）：写了项目层 network 后，两侧 Bash 里的
// 代理与 NO_PROXY 都是文件值。Windows 环境键大小写不敏感，TS 依次写大写 / 小写键，后写的小写键胜出，
// 所以比较完整的 `env | grep -i proxy` 而不是只读 `$HTTPS_PROXY`（曾因此误判为「Bash 不读配置」）。
test("project config network section reaches Bash subprocess env in both runtimes", async () => {
  const proxyUrl = "http://127.0.0.1:9";
  const observed: Record<string, string> = {};
  for (const kind of ["node", "rust"] as const) {
    const root = await mkdtemp(join(tmpdir(), `zcode-bash-proxy-${kind}-`));
    await writeNetwork(join(root, "workspace", ".zcode"), proxyUrl);
    let output = "missing";
    const bash = (request: any, response: any) => {
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      const last = request.messages.at(-1);
      if (last.role === "tool") {
        output = String(last.content);
        event(response, { content: "done" });
        end(response, "stop");
        return;
      }
      event(response, {
        tool_calls: [
          {
            index: 0,
            id: "bash-env",
            type: "function",
            function: {
              name: "Bash",
              arguments: JSON.stringify({
                command: "env | grep -iE '^(https?|all|no)_proxy=' | sort -f; echo END",
              }),
            },
          },
        ],
      });
      end(response, "tool_calls");
    };
    const f = await fixture(
      kind === "node"
        ? {
            root,
            mode: "yolo",
            registry: true,
            respond: bash,
            command: process.execPath,
            args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          }
        : { root, mode: "yolo", registry: true, respond: bash },
    );
    try {
      await configureRegistry(f, false);
      const h = f.start();
      const sid = await h.create();
      await h.subscribe(`conversation/${sid}`);
      await h.command(h.envelope("sendText", sid, { text: "print proxy env" }));
      await h.completed(sid);
      // 键名按小写比较：Windows 上 TS 先写大写再写小写键（大小写不敏感，后写者胜出），Rust 保留大写键，
      // 见 docs/specs/rust-net-proxy.md 已知差异；值必须逐字一致。
      observed[kind] = output
        .slice(0, output.indexOf("END"))
        .trim()
        .split("\n")
        .map((line) => line.replace(/^[^=]+/, (key) => key.toLowerCase()))
        .sort()
        .join("\n");
    } finally {
      await f.close();
    }
  }
  assert.ok(observed.node?.includes(`https_proxy=${proxyUrl}`), observed.node);
  assert.deepEqual(observed.rust, observed.node);
});

// WebFetch 走会话 app 的 httpClientPort（带 workingDirectory 的合并视图）：只写项目层 network，
// 两侧 WebFetch 都必须经代理（http 目标被升级为 https，代理收到 CONNECT 即算经过）。
test("project config network section routes WebFetch through the proxy in both runtimes", async () => {
  let hits = 0;
  const proxy = createServer((_req, res) => {
    hits += 1;
    res.writeHead(502);
    res.end();
  });
  proxy.on("connect", (_req, socket) => {
    hits += 1;
    socket.destroy();
  });
  proxy.listen(0, "127.0.0.1");
  await once(proxy, "listening");
  const proxyAddress = proxy.address();
  assert(proxyAddress && typeof proxyAddress !== "string");
  const proxyUrl = `http://127.0.0.1:${proxyAddress.port}`;
  const observed: string[] = [];
  try {
    for (const kind of ["node", "rust"] as const) {
      const root = await mkdtemp(join(tmpdir(), `zcode-webfetch-proxy-${kind}-`));
      await writeNetwork(join(root, "workspace", ".zcode"), proxyUrl);
      let done = false;
      const fetchOnce = (request: any, response: any) => {
        response.writeHead(200, { "Content-Type": "text/event-stream" });
        if (request.messages.at(-1).role === "tool") {
          done = true;
          event(response, { content: "done" });
          end(response, "stop");
          return;
        }
        event(response, {
          tool_calls: [
            {
              index: 0,
              id: "fetch",
              type: "function",
              function: {
                name: "WebFetch",
                arguments: JSON.stringify({
                  url: "http://zcode-proxy-probe.example/",
                  prompt: "what",
                }),
              },
            },
          ],
        });
        end(response, "tool_calls");
      };
      const f = await fixture(
        kind === "node"
          ? {
              root,
              mode: "yolo",
              registry: true,
              respond: fetchOnce,
              command: process.execPath,
              args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
            }
          : { root, mode: "yolo", registry: true, respond: fetchOnce },
      );
      try {
        await configureRegistry(f, false);
        const before = hits;
        const h = f.start();
        const sid = await h.create();
        await h.subscribe(`conversation/${sid}`);
        await h.command(h.envelope("sendText", sid, { text: "fetch it" }));
        const deadline = Date.now() + 30000;
        while (!done && Date.now() < deadline) await new Promise((r) => setTimeout(r, 100));
        observed.push(`${kind}:${hits > before ? "proxied" : "direct"}`);
      } finally {
        await f.close();
      }
    }
    assert.deepEqual(observed, ["node:proxied", "rust:proxied"]);
  } finally {
    proxy.closeAllConnections();
    await new Promise<void>((r) => proxy.close(() => r()));
  }
});

test("MCP handshake does not block the actor; stop cancels discovery and reaps the server before finishing", async () => {
  const f = await fixture({ mode: "yolo", respond });
  try {
    const server = await stdioServer(f.root, "slow-connect");
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, {
        workspaceId: f.cwd,
        mcpServers: [{ ...server.config, protocolVersion: "legacy" }],
      }),
    );
    const sid = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await h.command(h.envelope("sendText", sid, { text: "connect" }));
    const pid = Number((await waitForFile(server.started)).trim());
    const start = Date.now();
    await h.client.request("runtime/capabilities", {}, z.unknown());
    assert.ok(Date.now() - start < 1000);
    await h.command(h.envelope("stop", sid));
    await h.wait((m) =>
      m.params?.frame?.payload?.deltas?.some(
        (d: any) => d.patch?.control?.phase === "completedInterrupted",
      ),
    );
    assert.equal(f.requests.length, 0);
    assert.throws(() => process.kill(pid, 0));
    await h.close();
  } finally {
    await f.close();
  }
});

test("MCP cancellation after dispatch does not replay the tool and waits for process cleanup", async () => {
  const f = await fixture({ mode: "yolo", respond });
  try {
    const server = await stdioServer(f.root, "slow-call");
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, {
        workspaceId: f.cwd,
        mcpServers: [{ ...server.config, protocolVersion: "legacy" }],
      }),
    );
    const sid = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await h.command(h.envelope("sendText", sid, { text: "run once" }));
    await waitForFile(server.calls);
    const pid = Number((await readFile(server.started, "utf8")).trim());
    await h.command(h.envelope("stop", sid));
    await h.wait((m) =>
      m.params?.frame?.payload?.deltas?.some(
        (d: any) => d.patch?.control?.phase === "completedInterrupted",
      ),
    );
    assert.equal(f.requests.length, 1);
    assert.throws(() => process.kill(pid, 0));
    assert.equal((await readFile(server.calls, "utf8")).trim().split("\n").length, 1);
    await h.close();
    const cold = f.start();
    await cold.subscribe(`conversation/${sid}`);
    assert.equal(f.requests.length, 1);
    await cold.close();
    assert.equal((await readFile(server.calls, "utf8")).trim().split("\n").length, 1);
  } finally {
    await f.close();
  }
});

test("MCP client_credentials failure does not expose the client secret", async () => {
  const f = await fixture({ mode: "yolo" });
  try {
    const h = f.start();
    const result = await listMcp(h, f.cwd, [
      {
        name: "oauth",
        type: "http",
        url: "http://127.0.0.1:1/mcp",
        headers: [],
        // client_credentials 已支持（rust-mcp-oauth.md）；server 不可达时连接失败，状态与 stderr 都不能带出 secret。
        oauth: {
          type: "client_credentials",
          clientId: "fixture-client",
          clientSecret: "fixture-secret-never-echo",
        },
      },
    ]);
    assert.equal(result.statuses.oauth?.status, "failed");
    assert.ok(!JSON.stringify(result).includes("fixture-secret-never-echo"));
    assert.ok(!h.stderr.includes("fixture-secret-never-echo"));
    await h.close();
  } finally {
    await f.close();
  }
});

test("Invalid persisted MCP configuration reports config_invalid without blocking the normal Agent loop", async () => {
  const { mkdir, writeFile } = await import("node:fs/promises");
  const { join } = await import("node:path");
  const f = await fixture({
    mode: "yolo",
    respond(req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      event(res, { content: "normal model remains available" });
      end(res, "stop");
    },
  });
  try {
    await mkdir(join(f.cwd, ".zcode"));
    await writeFile(
      join(f.cwd, ".zcode/config.json"),
      JSON.stringify({
        mcp: {
          servers: {
            broken: { type: "http", url: "${MISSING_URL}" },
            disabled: { type: "stdio", enabled: false },
          },
        },
      }),
    );
    const h = f.start(),
      sid = await h.create();
    await h.subscribe(`conversation/${sid}`);
    await h.command(h.envelope("sendText", sid, { text: "normal" }));
    await h.completed(sid);
    const status = await listMcp(h, f.cwd, undefined, "status");
    assert.equal(status.statuses.broken?.failureKind, "config_invalid");
    assert.equal(status.statuses.disabled?.status, "disabled");
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});
