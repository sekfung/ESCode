import assert from "node:assert/strict";
import test from "node:test";
import { join } from "node:path";
import { access } from "node:fs/promises";
import { fixture, event, end, waitForFile } from "./zcode-cli-rust-fixture.js";
import { assertBeatStopped, waitForBeat } from "./zcode-cli-rust-shell-probe.js";

type Message = Record<string, any>;
// POSIX：job control 下 TERM-ignoring 的子进程；Windows：同构的两层心跳（父 + 子各写自己的文件）。
const command = `set -m; /bin/bash -c 'trap "" TERM; echo $$ > child.pid; while :; do sleep 0.05; done' & echo $$ > root.pid; wait`;
const heartbeatTreeCommand = `( for i in $(seq 1 200); do date +%s%N >> child.beat; sleep 0.05; done; echo leaked > child-leaked.txt ) & for i in $(seq 1 200); do date +%s%N >> root.beat; sleep 0.05; done`;
const isWindows = process.platform === "win32";
function gone(pid: number) {
  assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
}
function clean(pids: number[]) {
  for (const pid of pids) {
    try {
      process.kill(-pid, "SIGKILL");
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
    }
  }
}
for (const action of ["stop", "startNow", "EOF", "EPIPE"] as const) {
  test(`Rust ${action} waits for TERM-ignoring descendants before settling`, async () => {
    const pids: number[] = [];
    const f = await fixture({
      mode: "yolo",
      respond(_req, res, n) {
        res.writeHead(200, { "content-type": "text/event-stream" });
        if (n === 1) {
          event(res, {
            tool_calls: [
              {
                index: 0,
                id: "tree",
                type: "function",
                function: {
                  name: "Bash",
                  arguments: JSON.stringify({
                    command: isWindows ? heartbeatTreeCommand : command,
                  }),
                },
              },
            ],
          });
          end(res, "tool_calls");
        } else {
          for (const pid of pids) gone(pid);
          event(res, { content: "next execution" });
          end(res, "stop");
        }
      },
    });
    try {
      const h = f.start();
      const id = await h.create();
      await h.subscribe(`conversation/${id}`);
      await h.command(h.envelope("sendText", id, { text: "start shell tree" }));
      if (isWindows) {
        // 父与子各自心跳：只有整棵树被回收，两个文件才会同时停止增长。
        await waitForBeat(f.cwd, "root.beat");
        await waitForBeat(f.cwd, "child.beat");
      } else {
        pids.push(Number(await waitForFile(join(f.cwd, "root.pid"))));
        pids.push(Number(await waitForFile(join(f.cwd, "child.pid"))));
      }
      const began = performance.now();
      const after = h.messages.length;
      if (action === "EOF") {
        await h.close();
      } else if (action === "EPIPE") {
        h.child.stdout.destroy();
        h.child.stdin.write(
          `${JSON.stringify({ id: "epipe-probe", method: "runtime/capabilities", params: {} })}\n`,
        );
        let timer: ReturnType<typeof setTimeout> | undefined;
        try {
          await Promise.race([
            h.exited,
            new Promise((_, reject) => {
              timer = setTimeout(
                () => reject(new Error("EPIPE did not finish process cleanup")),
                4000,
              );
            }),
          ]);
        } finally {
          clearTimeout(timer);
        }
        await h.close();
      } else {
        const ack = await h.command(
          h.envelope(
            action === "stop" ? "stop" : "sendText",
            id,
            action === "stop" ? {} : { text: "new execution", requestedDelivery: "startNow" },
          ),
        );
        assert.equal(ack.status, "accepted");
        if (action === "startNow") await h.completed(id, after);
        else
          await h.wait(
            (m) =>
              m.params?.frame?.payload?.deltas?.some(
                (d: Message) => d.patch?.control?.phase === "completedInterrupted",
              ),
            after,
          );
      }
      assert(performance.now() - began < 3500, "termination exceeded its grace and cleanup budget");
      if (isWindows) {
        await assertBeatStopped(f.cwd, "root.beat");
        await assertBeatStopped(f.cwd, "child.beat");
        assert.equal(
          await access(join(f.cwd, "child-leaked.txt")).then(
            () => true,
            () => false,
          ),
          false,
        );
      } else {
        for (const pid of pids) gone(pid);
      }
      assert.equal(f.requests.length, action === "startNow" ? 2 : 1);
      assert.deepEqual(h.schemaErrors, []);
    } finally {
      clean(pids);
      await f.close();
    }
  });
}

test("Rust TaskStop waits for cross-group workers and exposes the committed background terminal state", async () => {
  const pids: number[] = [];
  let taskId = "";
  const f = await fixture({
    mode: "yolo",
    respond(req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      const last = req.messages.at(-1);
      if (last.role === "user") {
        const stop = last.content === "stop";
        event(res, {
          tool_calls: [
            {
              index: 0,
              id: stop ? "stop-tree" : "background-tree",
              type: "function",
              function: {
                name: stop ? "TaskStop" : "Bash",
                arguments: JSON.stringify(
                  stop ? { task_id: taskId } : { command, run_in_background: true },
                ),
              },
            },
          ],
        });
        end(res, "tool_calls");
      } else {
        if (last.tool_call_id === "background-tree")
          taskId = JSON.parse(last.content).backgroundTaskId;
        else for (const pid of pids) gone(pid);
        event(res, { content: "complete" });
        end(res, "stop");
      }
    },
  });
  try {
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "start" }));
    await h.completed(id);
    pids.push(Number(await waitForFile(join(f.cwd, "root.pid"))));
    pids.push(Number(await waitForFile(join(f.cwd, "child.pid"))));
    const after = h.messages.length;
    const began = performance.now();
    await h.command(h.envelope("sendText", id, { text: "stop" }));
    await h.completed(id, after);
    assert(performance.now() - began < 3500);
    for (const pid of pids) gone(pid);
    const frames = h.messages.slice(after).flatMap((m) => m.params?.frame?.payload?.deltas ?? []);
    assert(frames.some((d: Message) => d.patch?.backgroundWorks?.length === 0));
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    clean(pids);
    await f.close();
  }
});
