import assert from "node:assert/strict";
import test from "node:test";
import { copyFile, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-browser-use.md「第 4 期细则」：子进程环境清洗、出网配置恢复与 CUA 凭据定向注入。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const PROBE_KEYS = [
  "NODE_ENV",
  "OTEL_SERVICE_NAME",
  "ZCODE_TELEMETRY_DEVICE_MID",
  "ZCODE_CUA_PERMISSION_BROKER_SOCKET",
  "ZCODE_CUA_PLUGIN_AUTHORITY",
  "ZCODE_CUA_PERMISSION_BROKER_REFRESH_MARKER",
  "ZCODE_CUA_PERMISSION_BROKER_TOKEN",
  "ZCODE_CUA_NODE_REPL_HOST",
  "ZCODE_PLUGIN_ID",
  "ZCODE_REMOTE_HTTP_PROXY",
  "ZCODE_TOOL_ENV_PASSTHROUGH_JSON",
  "HTTP_PROXY",
  "HTTPS_PROXY",
  "ALL_PROXY",
  "NO_PROXY",
  "SSL_CERT_DIR",
  "NODE_EXTRA_CA_CERTS",
  "npm_config_proxy",
  "MCP_PROBE",
];
// 模型 fixture 在 127.0.0.1：`ZCODE_NO_PROXY` 让两侧模型请求绕过 `ZCODE_HTTP_PROXY`。
const RUNTIME_ENV: Record<string, string> = {
  NODE_ENV: "production",
  OTEL_SERVICE_NAME: "zcode-agent",
  ZCODE_TELEMETRY_DEVICE_MID: "device",
  ZCODE_CUA_PERMISSION_BROKER_SOCKET: "/tmp/cua.sock",
  ZCODE_CUA_PLUGIN_AUTHORITY: "authority",
  ZCODE_CUA_PERMISSION_BROKER_REFRESH_MARKER: "marker",
  ZCODE_CUA_PERMISSION_BROKER_TOKEN: "legacy-token",
  ZCODE_REMOTE_HTTP_PROXY: "http://remote",
  ZCODE_HTTP_PROXY: "127.0.0.1:9",
  ZCODE_NO_PROXY: "127.0.0.1,localhost",
  ZCODE_TOOL_ENV_PASSTHROUGH_JSON: JSON.stringify({ SSL_CERT_DIR: "/passthrough-certs" }),
  HTTPS_PROXY: "http://user-proxy",
  HTTP_PROXY: "http://user-proxy",
  NODE_EXTRA_CA_CERTS: "/user-ca.pem",
  npm_config_proxy: "http://npm-proxy",
};
const probeSource = (keys: string[]) =>
  `JSON.stringify(Object.fromEntries(${JSON.stringify(keys)}.map((k) => [k, process.env[k] ?? null])))`;

function toolCall(name: string, args: unknown) {
  return {
    tool_calls: [
      {
        index: 0,
        id: `c-${name}`,
        type: "function",
        function: { name, arguments: JSON.stringify(args) },
      },
    ],
  };
}

function launch(
  kind: "node" | "rust",
  respond: any,
  env: Record<string, string>,
  bundle = nodeBundle,
) {
  return kind === "node"
    ? fixture({
        command: process.execPath,
        args: ({ cwd }) => [bundle, "app-server", "--stdio", "--cwd", cwd],
        registry: true,
        respond,
        mode: "yolo",
        env,
      })
    : fixture({ registry: true, respond, mode: "yolo", env });
}

async function observeTools(kind: "node" | "rust") {
  const results: unknown[] = [];
  let f!: Awaited<ReturnType<typeof fixture>>;
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const done = req.messages.filter((m: any) => m.role === "tool");
    if (done.length) results.push(done.at(-1).content);
    const probe = join(f.root, "probe.mjs").replace(/\\/g, "/");
    if (done.length === 0)
      event(res, toolCall("Bash", { command: `node "${probe}"`, description: "probe" }));
    else if (done.length === 1) event(res, toolCall("mcp__probe__env", {}));
    else {
      event(res, { content: "done" });
      return end(res, "stop");
    }
    end(res, "tool_calls");
  };
  f = await launch(kind, respond, RUNTIME_ENV);
  try {
    await configureRegistry(f, false);
    await writeFile(join(f.root, "probe.mjs"), `console.log(${probeSource(PROBE_KEYS)});\n`);
    const server = join(f.root, "probe-mcp.mjs");
    await writeFile(
      server,
      `import { createInterface } from "node:readline";
const reply = (m, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }) + "\\n");
createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line);
  if (m.method === "initialize") reply(m, { protocolVersion: "2025-11-25", serverInfo: { name: "probe", version: "1" }, capabilities: { tools: {} } });
  else if (m.method === "tools/list") reply(m, { tools: [{ name: "env", inputSchema: { type: "object", properties: {} } }] });
  else if (m.method === "tools/call") reply(m, { content: [{ type: "text", text: ${probeSource(PROBE_KEYS)} }] });
  else if (m.id !== undefined) process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "no" } }) + "\\n");
});
`,
    );
    const h = f.start();
    const mcpServers = [
      {
        name: "probe",
        command: process.execPath,
        args: [server],
        env: [{ name: "MCP_PROBE", value: "scoped" }],
        protocolVersion: "auto",
        timeoutMs: 5000,
      },
    ];
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers }),
    );
    const sid = (ack.result as any).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await h.command(h.envelope("sendText", sid, { text: "probe" }));
    await h.completed(sid);
    await h.close();
    return results.map((r) => JSON.parse(String(r)));
  } finally {
    await f.close();
  }
}

test("Bash and MCP stdio children see the same sanitized environment as Node", async () => {
  const node = await observeTools("node");
  const rust = await observeTools("rust");
  assert.equal(node.length, 2);
  // 凭据、遥测与 NODE_ENV 被删；代理由 ZCODE_HTTP_PROXY 统一改写；透传 JSON 恢复后自身被删。
  assert.equal(node[0].ZCODE_CUA_PERMISSION_BROKER_SOCKET, null);
  assert.equal(node[0].HTTPS_PROXY, "http://127.0.0.1:9");
  assert.equal(node[1].MCP_PROBE, "scoped");
  // 有意差异：TS 经透传把遗留 broker token 恢复给工具子进程（与其「一并剔除」的注释矛盾），Rust 不恢复。
  const token = (seen: any[]) =>
    seen.map(({ ZCODE_CUA_PERMISSION_BROKER_TOKEN: value, ...rest }) => ({ value, rest }));
  assert.deepEqual(
    token(node).map((t) => t.value),
    ["legacy-token", "legacy-token"],
  );
  assert.deepEqual(
    token(rust).map((t) => t.value),
    [null, null],
  );
  assert.deepEqual(
    token(rust).map((t) => t.rest),
    token(node).map((t) => t.rest),
  );
});

// Computer Use：官方 computer-use 插件启用时，CUA 凭据只注入 node_repl；插件自带的旧 CUA MCP 与
// 用户配置里 CUA 形态的 server 退役；同名用户 node_repl 不能替换内置宿主。
const CUA_KEYS = [
  "ZCODE_CUA_PERMISSION_BROKER_SOCKET",
  "ZCODE_CUA_PLUGIN_AUTHORITY",
  "ZCODE_CUA_PERMISSION_BROKER_REFRESH_MARKER",
  "ZCODE_CUA_NODE_REPL_HOST",
  "ZCODE_PLUGIN_ID",
  "OTEL_SERVICE_NAME",
];

async function officialBase(root: string) {
  // 官方插件按 base 的相对候选路径发现：<base>/../node-repl-host 与 <base>/../zcode-cua-plugin。
  // Rust 读 ZCODE_OFFICIAL_PLUGINS_BASE_DIR；TS 以入口文件目录为 base，因此 Node 从同一 base 下的 bundle 副本启动。
  const base = join(root, "official", "dist");
  await mkdir(base, { recursive: true });
  await copyFile(nodeBundle, join(base, "zcode.cjs"));
  await symlink(join(dirname(nodeBundle), "provider"), join(base, "provider"), "junction");
  await symlink(
    resolve("apps/zcode-cli/packages/node-repl-host"),
    join(root, "official", "node-repl-host"),
    "junction",
  );
  const cua = join(root, "official", "zcode-cua-plugin");
  for (const dir of [".zcode-plugin", "docs", "scripts", "skills/computer-use"]) {
    await mkdir(join(cua, dir), { recursive: true });
  }
  await writeFile(
    join(cua, ".zcode-plugin", "plugin.json"),
    JSON.stringify({ name: "computer-use", version: "0.6.3" }),
  );
  await writeFile(join(cua, "docs", "computer-use.md"), "# Computer Use\n");
  await writeFile(join(cua, "scripts", "computer-use-client.mjs"), "export {};\n");
  await writeFile(
    join(cua, "skills", "computer-use", "SKILL.md"),
    "---\nname: computer-use\ndescription: Control the desktop.\n---\nUse node_repl.\n",
  );
  // 旧版 CUA 插件自带的 MCP：宿主写入官方插件 id 后按退役处理，不会启动。
  await writeFile(
    join(cua, ".mcp.json"),
    JSON.stringify({ mcpServers: { "computer-use": { command: "missing-legacy-cua", args: [] } } }),
  );
  return base;
}

async function observeCua(kind: "node" | "rust") {
  let definitions: string[] = [];
  let result: unknown;
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    definitions = (req.tools ?? [])
      .map((t: any) => t.function.name)
      .filter((n: string) => n.startsWith("mcp__"))
      .sort();
    if (req.messages.at(-1).role !== "tool") {
      event(
        res,
        toolCall("mcp__node_repl__js", {
          title: "Env",
          code: `console.log(${probeSource(CUA_KEYS)})`,
        }),
      );
      end(res, "tool_calls");
    } else {
      result = req.messages.at(-1).content;
      event(res, { content: "done" });
      end(res, "stop");
    }
  };
  const root = await mkdtemp(join(tmpdir(), "zcode-child-env-"));
  const base = await officialBase(root);
  const bundle = join(base, "zcode.cjs");
  const env = {
    ...RUNTIME_ENV,
    ZCODE_OFFICIAL_PLUGINS_BASE_DIR: base,
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ZCODE_PLUGIN_HOST_ENTRYPOINT: bundle,
  };
  const f = await launch(kind, respond, env, bundle);
  try {
    await configureRegistry(f, false);
    await mkdir(join(f.root, ".zcode", "cli"), { recursive: true });
    await writeFile(
      join(f.root, ".zcode", "cli", "config.json"),
      JSON.stringify({
        plugins: { enabledPlugins: { "computer-use@zcode-plugins-official": true } },
      }),
    );
    const h = f.start();
    const mcpServers = [
      {
        name: "node_repl",
        command: process.execPath,
        args: ["-e", "setInterval(() => {}, 1000)"],
        env: [],
      },
      { name: "legacy", command: "uvx", args: ["--from", "zcode-cua", "zcode-cua"], env: [] },
    ];
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers }),
    );
    const sid = (ack.result as any).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await h.command(h.envelope("sendText", sid, { text: "env" }));
    await h.completed(sid);
    await h.close();
    const text = String(result);
    return {
      definitions,
      env: JSON.parse(text.slice(text.indexOf("{"), text.lastIndexOf("}") + 1)),
    };
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Computer Use credentials reach only node_repl, and retired CUA servers stay hidden, as in Node", async () => {
  const node = await observeCua("node");
  const rust = await observeCua("rust");
  assert.equal(node.env.ZCODE_CUA_NODE_REPL_HOST, "1");
  assert.ok(node.definitions.includes("mcp__node_repl__js"));
  assert.deepEqual(rust, node);
});
