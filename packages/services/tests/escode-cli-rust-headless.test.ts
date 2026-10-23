import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { binary, fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-headless-prompt.md：同一模型夹具与 Provider Registry 下分别运行 Node `escode.cjs -p` 与 Rust `-p`，
// 比较 stdout / stderr / 退出码。各 runtime 用独立的 HOME（会话库互不影响），模型回复只由请求内容决定。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
type Runtime = "node" | "rust";
const RUN_TIMEOUT_MS = 60_000;

const text = (content: unknown) =>
  typeof content === "string" ? content : JSON.stringify(content);

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
  const tool = input.includes("read it")
    ? "Read"
    : input.includes("write it")
      ? "Write"
      : undefined;
  if (tool) {
    event(response, { content: "let me look. " });
    const args = tool === "Read" ? { file_path: "a.txt" } : { file_path: "b.txt", content: "x" };
    event(response, {
      tool_calls: [
        {
          index: 0,
          id: "c1",
          type: "function",
          function: { name: tool, arguments: JSON.stringify(args) },
        },
      ],
    });
    end(response, "tool_calls");
    return;
  }
  event(response, { content: `echo: ${input.slice(0, 40)}` });
  end(response, "stop");
}

async function setup() {
  const root = await mkdtemp(join(tmpdir(), "escode-headless-"));
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
          ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: join(root, "builtin.json"),
          ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: join(root, "personal.json"),
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
        done({
          code,
          stdout,
          stderr: stderr.replace(/^\(node:\d+\).*\n(.*trace-deprecation.*\n)?/gm, ""),
        });
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

/** 同一 runtime 依次执行全部参数组合（`-c` 依赖同一 HOME 的上一轮），返回可冻结的观察。 */
async function observe(kind: Runtime) {
  const { f, root, run } = await setup();
  try {
    const text = await run(kind, ["-p", "hi there"]);
    const json: { args: string; code: number | null; output: ReturnType<typeof comparable> }[] = [];
    const sessions: string[] = [];
    for (const args of [
      ["-p", "plain json", "--output-format", "json"],
      ["-p", "please read it", "--json"],
      // 缺省 yolo：Write 直接执行；显式 build：需要审批，headless 按 Node deny broker 拒绝（模型看到同一句话）。
      ["-p", "please write it", "--mode", "build", "--output-format", "json"],
      ["-p", "please write it", "--output-format", "json"],
      ["-p", "again", "-c", "--output-format", "json"],
    ]) {
      const result = await run(kind, args);
      json.push({ args: args.join(" "), code: result.code, output: comparable(result.stdout) });
      sessions.push(JSON.parse(result.stdout).sessionId);
    }
    const errors: { args: string; code: number | null; stderrFirst: string; stdout: string }[] = [];
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
      const result = await run(kind, args);
      errors.push({
        args: args.join(" "),
        code: result.code,
        stderrFirst: result.stderr.split("\n")[0],
        stdout: result.stdout,
      });
    }
    return {
      // text / json / 带工具的轮次 / 审批被拒的工具 / -c 续接，逐项冻结。
      text: { code: text.code, stdout: text.stdout, stderr: text.stderr },
      json,
      // -c 续接上一个会话：第 5 条 json 的 sessionId 与第 4 条相同。
      continuationSame: sessions[4] === sessions[3],
      errors,
    };
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("headless -p prints the same results and errors on Node and Rust", async () => {
  const node = await nodeOracle("runs", () => observe("node"));
  const rust = canonical(await observe("rust"));
  assert.equal(node.text.code, 0, node.text.stderr);
  assert.deepEqual(rust.text, node.text);
  for (const entry of node.json) assert.equal(entry.code, 0, entry.args);
  for (const entry of rust.json) assert.equal(entry.code, 0, entry.args);
  assert.deepEqual(
    rust.json.map((entry) => entry.output),
    node.json.map((entry) => entry.output),
  );
  assert.equal(node.continuationSame, true);
  assert.equal(rust.continuationSame, node.continuationSame);
  assert.deepEqual(rust.errors, node.errors);
});
