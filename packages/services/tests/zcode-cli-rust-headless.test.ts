import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { binary, fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-headless-prompt.md：同一模型夹具与 Provider Registry 下分别运行 Node `zcode.cjs -p` 与 Rust `-p`，
// 比较 stdout / stderr / 退出码。各 runtime 用独立的 HOME（会话库互不影响），模型回复只由请求内容决定。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
type Runtime = "node" | "rust";
const RUN_TIMEOUT_MS = 60_000;

const text = (content: unknown) => (typeof content === "string" ? content : JSON.stringify(content));

/** 无状态的模型回复：要工具时先调 Read / Write，工具结果之后给最终回答，否则回显输入。 */
function respond(request: any, response: any) {
  response.writeHead(200, { "content-type": "text/event-stream" });
  const last = request.messages.at(-1);
  if (last.role === "tool") {
    event(response, { content: `after tool: ${text(last.content).slice(0, 40)}` });
    end(response, "stop");
    return;
  }
  const input = text(last.content);
  const tool = input.includes("read it") ? "Read" : input.includes("write it") ? "Write" : undefined;
  if (tool) {
    event(response, { content: "let me look. " });
    const args = tool === "Read" ? { file_path: "a.txt" } : { file_path: "b.txt", content: "x" };
    event(response, {
      tool_calls: [{ index: 0, id: "c1", type: "function", function: { name: tool, arguments: JSON.stringify(args) } }],
    });
    end(response, "tool_calls");
    return;
  }
  event(response, { content: `echo: ${input.slice(0, 40)}` });
  end(response, "stop");
}

async function setup() {
  const root = await mkdtemp(join(tmpdir(), "zcode-headless-"));
  const f = await fixture({ root, registry: true, respond });
  await configureRegistry(f);
  const homes = { node: join(root, "home-node"), rust: join(root, "home-rust") };
  const workspaces = { node: join(homes.node, "ws"), rust: join(homes.rust, "ws") };
  for (const ws of Object.values(workspaces)) {
    await mkdir(ws, { recursive: true });
    await writeFile(join(ws, "a.txt"), "hello from file");
  }
  const run = (kind: Runtime, args: string[]) =>
    new Promise<{ code: number | null; stdout: string; stderr: string }>((done) => {
      const command = kind === "node" ? process.execPath : binary;
      const argv = kind === "node" ? [nodeBundle, ...args] : args;
      const child = spawn(command, [...argv, "--cwd", workspaces[kind]], {
        cwd: workspaces[kind],
        env: {
          ...process.env,
          HOME: homes[kind],
          USERPROFILE: homes[kind],
          ZCODE_BUILTIN_PROVIDER_CONFIG_FILE: join(root, "builtin.json"),
          ZCODE_PERSONAL_PROVIDER_CONFIG_FILE: join(root, "personal.json"),
          // 模型夹具在 127.0.0.1；开发机的 HTTP(S)_PROXY 不能接管本地请求（同 fixture）。
          NO_PROXY: "127.0.0.1,localhost",
          no_proxy: "127.0.0.1,localhost",
        },
      });
      let stdout = "";
      let stderr = "";
      child.stdout.on("data", (d) => (stdout += d));
      child.stderr.on("data", (d) => (stderr += d));
      const timer = setTimeout(() => child.kill(), RUN_TIMEOUT_MS);
      child.on("close", (code) => {
        clearTimeout(timer);
        done({ code, stdout, stderr: stderr.replace(/^\(node:\d+\).*\n(.*trace-deprecation.*\n)?/gm, "") });
      });
    });
  return { f, root, run };
}

/** json 输出的可比部分：去掉 id、eventCount 与 contextUsed（spec 已知差异），保留键序与其余值。 */
function comparable(stdout: string) {
  const value = JSON.parse(stdout);
  return {
    keys: Object.keys(value),
    response: value.response,
    usage: value.usage,
    projectionKeys: Object.keys(value.projection),
    projection: { ...value.projection, contextUsed: typeof value.projection.contextUsed },
    sessionIdShape: /^sess_[0-9a-f-]{36}$/.test(value.sessionId),
  };
}

test("headless -p prints the same results and errors on Node and Rust", async () => {
  const { f, root, run } = await setup();
  try {
    // text / json / 带工具的轮次 / 审批被拒的工具 / -c 续接，两侧逐项比较。
    for (const args of [["-p", "hi there"]]) {
      const [node, rust] = [await run("node", args), await run("rust", args)];
      assert.equal(node.code, 0, node.stderr);
      assert.deepEqual(rust, node);
    }
    const sessions: Record<Runtime, string[]> = { node: [], rust: [] };
    for (const args of [
      ["-p", "plain json", "--output-format", "json"],
      ["-p", "please read it", "--json"],
      // 缺省 yolo：Write 直接执行；显式 build：需要审批，headless 按 Node deny broker 拒绝（模型看到同一句话）。
      ["-p", "please write it", "--mode", "build", "--output-format", "json"],
      ["-p", "please write it", "--output-format", "json"],
      ["-p", "again", "-c", "--output-format", "json"],
    ]) {
      const results = { node: await run("node", args), rust: await run("rust", args) };
      assert.equal(results.node.code, 0, `${args.join(" ")}: ${results.node.stderr}`);
      assert.equal(results.rust.code, 0, `${args.join(" ")}: ${results.rust.stderr}`);
      assert.deepEqual(comparable(results.rust.stdout), comparable(results.node.stdout), args.join(" "));
      for (const kind of ["node", "rust"] as const) sessions[kind].push(JSON.parse(results[kind].stdout).sessionId);
    }
    // -c 续接上一个会话（两侧同样）。
    for (const kind of ["node", "rust"] as const) assert.equal(sessions[kind][4], sessions[kind][3], kind);
    // 参数错误：退出码与 stderr 首行一致。
    for (const args of [
      ["-p", ""],
      ["-p", "x", "--output-format", "xml"],
      ["-p", "x", "--mode", "bad"],
      ["-p", "x", "--resume", "a", "-c"],
      ["-p", "x", "--locale", "fr"],
      ["-p", "x", "--bogus"],
      ["-p", "x", "--disallowed-tools"],
      ["-p"],
    ]) {
      const [node, rust] = [await run("node", args), await run("rust", args)];
      const first = (s: string) => s.split("\n")[0];
      assert.equal(rust.code, node.code, args.join(" "));
      assert.equal(first(rust.stderr), first(node.stderr), args.join(" "));
      assert.equal(rust.stdout, node.stdout, args.join(" "));
    }
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
});
