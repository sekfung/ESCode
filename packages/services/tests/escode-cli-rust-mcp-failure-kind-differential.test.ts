import assert from "node:assert/strict";
import test from "node:test";
import { createServer, type ServerResponse } from "node:http";
import { once } from "node:events";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { escodeMcpListResultSchema, type ESCodeProtocolMcpServer } from "@escode/shared";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-mcp-parity.md「传输失败分类」：Node 先按传输设置默认 failureKind
// （stdio = process_start_failed，http/sse = network_unreachable），只有版本协商类错误才
// 改写为 protocol_negotiation_failed。Rust 必须产出同一分类，不能把进程退出、JSON-RPC
// 拒绝或首连断流误报成协商失败。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function closedPort(): Promise<number> {
  const server = createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  await new Promise<void>((done) => server.close(() => done()));
  return port;
}

async function observe(kind: "node" | "rust", config: ESCodeProtocolMcpServer) {
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ registry: true });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const listed = await h.client.request(
      "mcp/list",
      {
        workspace: { workspacePath: f.cwd, workspaceKey: f.cwd },
        mcpServers: [config],
        mode: "connect",
      },
      escodeMcpListResultSchema,
    );
    await h.close();
    const status = listed.statuses[config.name];
    return { status: status?.status, failureKind: status?.failureKind };
  } finally {
    await f.close();
  }
}

type Case = {
  config: ESCodeProtocolMcpServer;
  close?: () => Promise<void>;
};

const stdioScripts: Record<string, string> = {
  "stdio-init-error": `
import { createInterface } from "node:readline";
const reply=(m,result)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result})+"\\n");
const fail=(m,code,message)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,error:{code,message}})+"\\n");
createInterface({input:process.stdin}).on("line",line=>{const m=JSON.parse(line);
 if(m.method==="server/discover")fail(m,-32601,"legacy");
 else if(m.method==="initialize")fail(m,-32000,"init denied");
});`,
  "stdio-init-bad-result": `
import { createInterface } from "node:readline";
const reply=(m,result)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result})+"\\n");
const fail=(m,code,message)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,error:{code,message}})+"\\n");
createInterface({input:process.stdin}).on("line",line=>{const m=JSON.parse(line);
 if(m.method==="server/discover")fail(m,-32601,"legacy");
 else if(m.method==="initialize")reply(m,{serverInfo:{name:"x",version:"1"}});
});`,
  "stdio-unsupported-version": `
import { createInterface } from "node:readline";
const reply=(m,result)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result})+"\\n");
const fail=(m,code,message)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,error:{code,message}})+"\\n");
createInterface({input:process.stdin}).on("line",line=>{const m=JSON.parse(line);
 if(m.method==="server/discover")fail(m,-32601,"legacy");
 else if(m.method==="initialize")reply(m,{protocolVersion:"1999-01-01",serverInfo:{name:"x",version:"1"},capabilities:{tools:{}}});
});`,
  "stdio-discover-empty-result": `
import { createInterface } from "node:readline";
const reply=(m,result)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result})+"\\n");
createInterface({input:process.stdin}).on("line",line=>{const m=JSON.parse(line);
 if(m.method==="server/discover")reply(m,{});
 else if(m.method==="initialize")reply(m,{protocolVersion:"2025-11-25",serverInfo:{name:"x",version:"1"},capabilities:{tools:{}}});
 else if(m.method==="tools/list")reply(m,{tools:[]});
});`,
};

async function caseFor(name: string): Promise<Case> {
  if (name in stdioScripts) {
    const root = await mkdtemp(join(tmpdir(), "mcp-failure-kind-"));
    const script = join(root, `${name}.mjs`);
    await writeFile(script, stdioScripts[name]!);
    return {
      config: {
        name: "local.echo",
        command: process.execPath,
        args: [script],
        env: [],
        timeoutMs: 10_000,
      },
      close: () => rm(root, { recursive: true, force: true }),
    };
  }
  if (name === "stdio-exit") {
    return {
      config: {
        name: "local.echo",
        command: process.execPath,
        args: ["-e", "process.exit(1)"],
        env: [],
        timeoutMs: 10_000,
      },
    };
  }
  if (name === "stdio-missing") {
    return {
      config: {
        name: "local.echo",
        command: "/nonexistent/escode-mcp",
        args: [],
        env: [],
        timeoutMs: 10_000,
      },
    };
  }
  if (
    name === "http-json-error" ||
    name === "http-unsupported-version" ||
    name === "http-discover-empty-result" ||
    name === "http-sse-discover-empty-result" ||
    name === "sse-drop" ||
    name === "sse-unsupported-version"
  ) {
    let stream: ServerResponse | undefined;
    let eventId = 0;
    const sse = name.startsWith("sse-");
    const sseReply = name === "http-sse-discover-empty-result";
    const emptyDiscover = name.endsWith("discover-empty-result");
    const server = createServer(async (req, res) => {
      if (req.method === "GET") {
        if (name === "sse-drop") {
          res.writeHead(200, { "Content-Type": "text/event-stream" }).end();
          return;
        }
        if (!sse) {
          res.writeHead(405).end();
          return;
        }
        stream = res;
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write("event: endpoint\ndata: /messages\n\n");
        return;
      }
      req.setEncoding("utf8");
      let body = "";
      for await (const part of req) body += part;
      if (name === "sse-drop") {
        res.writeHead(200, { "Content-Type": "text/event-stream" }).end();
        return;
      }
      const message = JSON.parse(body || "{}");
      const response =
        name === "http-json-error"
          ? {
              jsonrpc: "2.0",
              id: message.id,
              error: { code: -32000, message: "denied" },
            }
          : message.method === "server/discover"
            ? emptyDiscover
              ? { jsonrpc: "2.0", id: message.id, result: {} }
              : { jsonrpc: "2.0", id: message.id, error: { code: -32601, message: "legacy" } }
            : message.method === "initialize"
              ? {
                  jsonrpc: "2.0",
                  id: message.id,
                  result: {
                    protocolVersion: emptyDiscover ? "2025-11-25" : "1999-01-01",
                    serverInfo: { name: "x", version: "1" },
                    capabilities: { tools: {} },
                  },
                }
              : { jsonrpc: "2.0", id: message.id, result: { tools: [] } };
      if (sse) {
        res.writeHead(202).end();
        stream?.write(`id: ${++eventId}\nevent: message\ndata: ${JSON.stringify(response)}\n\n`);
      } else if (sseReply) {
        res
          .writeHead(200, { "Content-Type": "text/event-stream" })
          .end(`event: message\ndata: ${JSON.stringify(response)}\n\n`);
      } else {
        res.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify(response));
      }
    });
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const address = server.address();
    const port = typeof address === "object" && address ? address.port : 0;
    return {
      config: {
        name: "remote",
        type: sse ? "sse" : "http",
        url: `http://127.0.0.1:${port}/${sse ? "sse" : "mcp"}`,
        headers: [],
        timeoutMs: 10_000,
      },
      close: async () => {
        stream?.end();
        server.closeAllConnections();
        await new Promise<void>((done) => server.close(() => done()));
      },
    };
  }
  const port = await closedPort();
  return {
    config: {
      name: "remote",
      type: name === "sse-refused" ? "sse" : "http",
      url: `http://127.0.0.1:${port}/${name === "sse-refused" ? "sse" : "mcp"}`,
      headers: [],
      timeoutMs: 10_000,
    },
  };
}

for (const name of [
  "http-refused",
  "sse-refused",
  "stdio-exit",
  "stdio-missing",
  "stdio-init-error",
  "stdio-init-bad-result",
  "stdio-unsupported-version",
  "stdio-discover-empty-result",
  "http-json-error",
  "http-unsupported-version",
  "http-discover-empty-result",
  "http-sse-discover-empty-result",
  "sse-drop",
  "sse-unsupported-version",
] as const)
  test(`MCP ${name} failureKind matches Node`, async () => {
    const current = await caseFor(name);
    try {
      const node = await nodeOracle(name, () => observe("node", current.config));
      const rust = canonical(await observe("rust", current.config));
      assert.deepEqual(rust, node);
    } finally {
      await current.close?.();
    }
  });
