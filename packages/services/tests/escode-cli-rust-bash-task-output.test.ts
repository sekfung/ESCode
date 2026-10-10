import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// TaskOutput 读后台 Bash：运行中非阻塞读（not_ready）与阻塞读到终态（success + exit code + 输出）。模型看到的是
// TS formatTaskOutputModelContent 的 XML 块（task id / 输出路径抹掉）。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const command = "sleep 2; printf 'line one\\nline two\\n'";

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [{ index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } }],
});
const toolResult = (req: any, id: string) =>
  String(req?.messages?.find((m: any) => m.role === "tool" && m.tool_call_id === id)?.content ?? "");

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  let taskId = "missing";
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const n = requests.length;
    if (n === 1) {
      event(res, call("bg-1", "Bash", { command, description: "print lines", run_in_background: true }));
      end(res, "tool_calls");
    } else if (n === 2) {
      taskId = /ID: (\S+?)\./.exec(toolResult(req, "bg-1"))?.[1] ?? "missing";
      event(res, call("to-1", "TaskOutput", { task_id: taskId, block: false, timeout: 0 }));
      end(res, "tool_calls");
    } else if (n === 3) {
      event(res, call("to-2", "TaskOutput", { task_id: taskId, block: true, timeout: 20000 }));
      end(res, "tool_calls");
    } else {
      event(res, { content: "done" });
      end(res, "stop");
    }
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ registry: true, respond, mode: "yolo" });
  try {
    await configureRegistry(f, false);
    const h: Harness = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "run and read", mode: "yolo" }));
    const deadline = Date.now() + 30_000;
    while (requests.length < 4 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.completed(id);
    await h.close();
    const scrub = (text: string) =>
      text
        .replaceAll(taskId, "<task>")
        .replace(/(?:[A-Za-z]:)?[\\/][^\s<"]*[\\/]([^\\/\s<"]+\.log)/g, "<dir>/$1");
    return {
      running: scrub(toolResult(requests[2], "to-1")),
      settled: scrub(toolResult(requests[3], "to-2")),
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust read a background Bash task with TaskOutput the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  if (process.env.DEBUG_TO) console.log(JSON.stringify({ node, rust }, null, 1));
  assert.deepEqual(node.schemaErrors, []);
  assert.match(node.settled, /<retrieval_status>success<\/retrieval_status>/);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
