import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// 后台 Bash 完成通知：`run_in_background` 的命令在回合结束后完成 → 一个后台结果轮，模型收到 `<task-notification>`
// （task id / 输出文件路径 / 耗时抹掉）。Node 的 BackgroundTaskTracker 对 local_bash 发这条通知。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const command = "sleep 2; echo hello-from-background";

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const n = requests.length;
    if (n === 1) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "bg-1",
            type: "function",
            function: {
              name: "Bash",
              arguments: JSON.stringify({ command, description: "say hello", run_in_background: true }),
            },
          },
        ],
      });
      end(res, "tool_calls");
    } else {
      event(res, { content: n === 2 ? "waiting" : "done" });
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
    await h.command(h.envelope("sendText", id, { text: "run it in the background", mode: "yolo" }));
    const deadline = Date.now() + 30_000;
    while (requests.length < 3 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.completed(id);
    await h.close();
    // 输出目录随存储根而异（Node 的 .escode/cli/exec/<session>，Rust 的数据目录）；比对文件名。
    const scrub = (text: string) =>
      text
        .replace(/(?:[A-Za-z]:)?[\\/][^\s<"]*[\\/]([^\\/\s<"]+\.log)/g, "<dir>/$1")
        .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<uuid>")
        .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>");
    const lastUser = (req: any) => {
      const content = req?.messages?.filter((m: any) => m.role === "user").at(-1)?.content;
      return scrub(typeof content === "string" ? content : String(JSON.stringify(content)));
    };
    const launch = requests[1]?.messages?.find((m: any) => m.tool_call_id === "bg-1")?.content;
    return {
      launch: scrub(String(launch)),
      notification: lastUser(requests[2]),
      requestCount: requests.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust notify the model when a background Bash command finishes", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.match(node.notification, /<task-notification>/);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
