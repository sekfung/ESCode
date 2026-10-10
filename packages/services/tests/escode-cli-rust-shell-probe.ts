import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

/**
 * 跨平台存活探测：shell 循环追加时间戳，进程被终止后文件不再增长。
 * 不依赖 POSIX PID 语义（Git Bash 的 $$ 是 MSYS pid，Node 的 process.kill 用 Windows pid）。
 */
export function shellHeartbeatCommand(
  beats = "shell.beat",
  leaked = "leaked.txt",
  ticks = 150,
): string {
  return `for i in $(seq 1 ${ticks}); do date +%s%N >> ${beats}; sleep 0.2; done; echo leaked > ${leaked}`;
}

/** 等到心跳至少写入 min 次，证明 shell 已在运行。 */
export async function waitForBeat(cwd: string, beats = "shell.beat", min = 2): Promise<number> {
  const started = Date.now();
  while (true) {
    const size = await readFile(join(cwd, beats), "utf8").then(
      (text) => text.split("\n").filter(Boolean).length,
      () => 0,
    );
    if (size >= min) return size;
    assert.ok(Date.now() - started < 5000, `No heartbeat from shell in ${cwd}`);
    await delay(20);
  }
}

/** 当前心跳条数，用于区分同一文件里的多次启动。 */
export async function beatCount(cwd: string, beats = "shell.beat"): Promise<number> {
  return readFile(join(cwd, beats), "utf8").then(
    (text) => text.split("\n").filter(Boolean).length,
    () => 0,
  );
}

/** 等到心跳相对先前条数继续增长，证明新一次启动已在运行。 */
export async function waitForBeatGrowth(cwd: string, beats: string, from: number): Promise<void> {
  const started = Date.now();
  while (true) {
    if ((await beatCount(cwd, beats)) > from) return;
    assert.ok(Date.now() - started < 5000, `No heartbeat growth from shell in ${cwd}`);
    await delay(20);
  }
}

/** 断言心跳已停止增长（进程或其子进程仍在跑时会增长）。 */
export async function assertBeatStopped(cwd: string, beats = "shell.beat"): Promise<void> {
  const size = await readFile(join(cwd, beats), "utf8").then(
    (t) => t.length,
    () => 0,
  );
  await delay(800);
  const after = await readFile(join(cwd, beats), "utf8").then(
    (t) => t.length,
    () => 0,
  );
  assert.equal(after, size, `Shell kept running after termination: ${cwd}`);
}
