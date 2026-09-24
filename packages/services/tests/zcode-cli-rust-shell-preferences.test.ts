import assert from "node:assert/strict";
import test from "node:test";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";

// docs/specs/rust-shell-selection.md 验收 2：Host 偏好决定 Windows Bash 的实际 shell，且每会话只请求一次。
function call(res: any, command: string, id: string) {
  event(res, {
    tool_calls: [
      {
        index: 0,
        id,
        type: "function",
        function: { name: "Bash", arguments: JSON.stringify({ command }) },
      },
    ],
  });
  end(res, "tool_calls");
}

async function runTwoBashCalls(shell?: Record<string, unknown>) {
  const outputs: string[] = [];
  let step = 0;
  const f = await fixture({
    respond(req, res) {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      const last = req.messages.at(-1);
      if (step > 0) outputs.push(String(last?.content ?? ""));
      switch (step++) {
        case 0:
          call(res, "echo %OS%", "first");
          break;
        case 1:
          call(res, "echo second", "second");
          break;
        default:
          event(res, { content: "done" });
          end(res, "stop");
      }
    },
  });
  try {
    const h = f.start();
    if (shell) h.integratedTerminalShell = shell;
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "run", mode: "yolo" }));
    await h.completed(id);
    assert.deepEqual(h.schemaErrors, []);
    return { outputs, requests: h.runtimePreferenceRequests, id };
  } finally {
    await f.close();
  }
}

test(
  "Windows Bash honours Host CMD selection and requests preferences once per session",
  { skip: process.platform !== "win32" },
  async () => {
    const { outputs, requests, id } = await runTwoBashCalls({
      mode: "shell",
      dialect: "cmd",
      id: "cmd",
      label: "CMD",
      path: "cmd.exe",
    });
    assert.match(outputs[0]!, /Windows_NT/);
    assert.deepEqual(requests, [{ sessionId: id, scope: "user-execution" }]);
  },
);

test(
  "Windows Bash defaults to Git Bash when Host selection is auto",
  { skip: process.platform !== "win32" },
  async () => {
    const { outputs } = await runTwoBashCalls();
    // Git Bash 不展开 %OS%，原样输出；cmd 会输出 Windows_NT。
    assert.match(outputs[0]!, /%OS%/);
  },
);
