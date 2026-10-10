import assert from "node:assert/strict";
import test from "node:test";
import { basename, resolve } from "node:path";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// `v4/conversation/backgroundBashOutput`：后台 Bash 详情面板的输出尾窗。运行中、结算后与未知 workId 三种应答
// 两侧一致（输出路径只比文件名）。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const command = "echo first; sleep 2; echo second";

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (requests.length === 1) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "bg-1",
            type: "function",
            function: { name: "Bash", arguments: JSON.stringify({ command, description: "two lines", run_in_background: true }) },
          },
        ],
      });
      end(res, "tool_calls");
    } else {
      event(res, { content: "ok" });
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
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    await h.command(h.envelope("sendText", sessionId, { text: "run", mode: "yolo" }));
    await h.completed(sessionId);
    const launch = String(requests[1]?.messages?.find((m: any) => m.tool_call_id === "bg-1")?.content ?? "");
    const workId = /ID: (\S+?)\./.exec(launch)?.[1] ?? "missing";
    const parse = { parse: (value: unknown) => value } as any;
    const read = async (id: string) => {
      const value = (await h.client.request("v4/conversation/backgroundBashOutput", { sessionId, workId: id }, parse)) as any;
      return {
        ...value,
        workId: value.workId === workId ? "<work>" : value.workId,
        ...(value.outputPath ? { outputPath: basename(value.outputPath) } : {}),
      };
    };
    // shell 启动耗时两侧不同：按状态轮询，而不是固定等待。
    const until = async (ok: (value: any) => boolean) => {
      const deadline = Date.now() + 20_000;
      let value = await read(workId);
      while (!ok(value) && Date.now() < deadline) {
        await new Promise((done) => setTimeout(done, 100));
        value = await read(workId);
      }
      return value;
    };
    const running = await until((v) => v.status === "running" && v.output === "first\n");
    const settled = await until((v) => v.status !== "running");
    const unknown = await read("exec_unknown");
    await h.close();
    return { running, settled, unknown, schemaErrors: h.schemaErrors };
  } finally {
    await f.close();
  }
}

test("Node and Rust read background Bash output for the details panel the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.equal(node.settled.status, "completed");
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
