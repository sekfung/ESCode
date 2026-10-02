import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { backgroundNotice } from "./zcode-cli-rust-bash-notice.js";

// TaskStop 停掉的后台 Bash：停止的工具结果就是模型对这次停止的全部所知，TS 不再发完成通知
// （否则模型会被一条「任务被 killed」再唤起一轮）。比对停止那一轮之后的模型请求。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  let taskId = "";
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const last = req.messages.at(-1);
    if (last.role === "tool" && last.tool_call_id === "bg") taskId = backgroundNotice(last.content).taskId;
    if (last.role === "user" && (last.content === "start" || last.content === "stop")) {
      const stop = last.content === "stop";
      const args = stop ? { task_id: taskId } : { command: "sleep 30", run_in_background: true };
      event(res, {
        tool_calls: [{ index: 0, id: stop ? "stop" : "bg", type: "function", function: { name: stop ? "TaskStop" : "Bash", arguments: JSON.stringify(args) } }],
      });
      end(res, "tool_calls");
      return;
    }
    event(res, { content: "ok" });
    end(res, "stop");
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
    let after = h.messages.length;
    await h.command(h.envelope("sendText", id, { text: "start", mode: "yolo" }));
    await h.completed(id, after);
    const before = requests.length;
    after = h.messages.length;
    await h.command(h.envelope("sendText", id, { text: "stop", mode: "yolo" }));
    await h.completed(id, after);
    await new Promise((done) => setTimeout(done, 2000));
    await h.close();
    return {
      requestsAfterStop: requests.length - before,
      lastRoles: requests.slice(before).map((r) => r.messages.at(-1).role),
      notified: requests.slice(before).some((r) => JSON.stringify(r.messages.at(-1)).includes("task-notification")),
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust do not notify about a background Bash stopped by TaskStop", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
});
