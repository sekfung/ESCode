import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// 后台子代理在父回合进行中完成：Node 在下一个步边界把完成通知并入本回合（task_notification_steer），而不是
// 等回合结束再开后台结果轮。比对父会话每次模型请求的尾部消息（agent id / 路径 / 耗时抹掉）。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [{ index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } }],
});

async function observe(kind: "node" | "rust") {
  const main: any[] = [];
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const first = req.messages.find((m: any) => m.role === "user" && !String(m.content).startsWith("<system-reminder>"));
    if (String(first?.content ?? "").includes("background child")) {
      event(res, { content: "child evidence" });
      end(res, "stop");
      return;
    }
    main.push(req);
    const n = main.length;
    if (n === 1) {
      event(res, call("launch", "Agent", { description: "background check", prompt: "background child", run_in_background: true }));
      end(res, "tool_calls");
    } else if (n === 2) {
      event(res, call("wait-1", "Bash", { command: "sleep 3; echo slept", description: "wait a bit" }));
      end(res, "tool_calls");
    } else {
      event(res, { content: n === 3 ? "noted" : "done" });
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
    await h.command(h.envelope("sendText", id, { text: "delegate", mode: "yolo" }));
    const deadline = Date.now() + 30_000;
    while (main.length < 3 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.completed(id);
    await new Promise((done) => setTimeout(done, 1500));
    await h.close();
    const scrub = (text: string) =>
      text
        // 输出目录随存储根而异；比对文件名。
        .replace(/<output-file>[^<]*[\\/]([^\\/<]+)<\/output-file>/g, "<output-file><dir>/$1</output-file>")
        .replace(/agent_[0-9a-f]{8}[A-Za-z0-9_-]*/g, "<agent>")
        .replace(/<duration_ms>\d+</g, "<duration_ms><n><")
        .replace(/(?:[A-Za-z]:)?[\\/][^\s<"]*[\\/]([^\\/\s<"]+)/g, "<dir>/$1")
        .replace(/\d+(\.\d+)?\s?(ms|s)\b/g, "<duration>");
    const tail = (req: any) =>
      (req?.messages ?? []).slice(-2).map((m: any) => ({
        role: m.role,
        content: scrub(typeof m.content === "string" ? m.content : String(JSON.stringify(m.content))),
      }));
    return {
      afterWait: tail(main[2]),
      requestCount: main.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust steer a background subagent completion into the running parent turn", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
