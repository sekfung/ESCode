import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createSettingService } from "@escode/services/node";

// docs/specs/rust-u2-host-skeleton.md §5 优先序 3 的验收：settings 域在 Node Host 与 Rust Host
// 之间**交替写读**同一 `<home>/.escode/v2/setting.json`——两侧同文件同格式，来回切换不破坏状态。
//
// 一侧是真实 TS 设置服务（createSettingService），另一侧是真实 `escode-host` 进程
// （控制通道的 settingsRead/settingsWrite 消息）。

const hostBinary = resolve("apps/escode-host/target/debug/escode-host");

interface HostChild {
  send(message: Record<string, unknown>): Promise<void>;
  next(): Promise<Record<string, unknown>>;
  stop(): Promise<void>;
}

function startHost(home: string): HostChild {
  const child = spawn(hostBinary, [], {
    stdio: ["pipe", "pipe", "inherit"],
    env: { ...process.env, HOME: home, USERPROFILE: home },
  });
  let buffer = "";
  const pending: Array<(value: Record<string, unknown>) => void> = [];
  const queue: Record<string, unknown>[] = [];
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    buffer += chunk;
    let newline: number;
    while ((newline = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      if (!line.trim()) continue;
      const message = JSON.parse(line) as Record<string, unknown>;
      const resolveNext = pending.shift();
      if (resolveNext) resolveNext(message);
      else queue.push(message);
    }
  });
  return {
    async send(message) {
      child.stdin.write(`${JSON.stringify(message)}\n`);
    },
    next() {
      const queued = queue.shift();
      if (queued) return Promise.resolve(queued);
      return new Promise((resolveNext) => pending.push(resolveNext));
    },
    async stop() {
      child.stdin.end();
      await new Promise<void>((done) => child.once("close", () => done()));
    },
  };
}

/// 读一条非日志帧（Host 的日志批次与就绪帧与本用例无关）。
async function nextMeaningful(host: HostChild): Promise<Record<string, unknown>> {
  for (let i = 0; i < 20; i += 1) {
    const message = await host.next();
    if (message.kind !== "logBatch") return message;
  }
  throw new Error("no meaningful control frame");
}

test("Node and Rust Hosts alternate writes to the same settings file", async (t) => {
  if (!existsSync(hostBinary)) {
    t.skip(`escode-host binary missing (${hostBinary}); build apps/escode-host first`);
    return;
  }
  const root = await mkdtemp(join(tmpdir(), "escode-settings-interop-"));
  const home = join(root, "home");
  await mkdir(home, { recursive: true });
  // TS 服务按调用时解析 HOME（不在导入时固化），测试进程内切换即可。
  const previousHome = process.env.ESCODE_DESKTOP_HOME_DIR;
  process.env.ESCODE_DESKTOP_HOME_DIR = home;
  t.after(() => {
    if (previousHome === undefined) delete process.env.ESCODE_DESKTOP_HOME_DIR;
    else process.env.ESCODE_DESKTOP_HOME_DIR = previousHome;
  });

  const settingsFile = join(home, ".escode", "v2", "setting.json");
  const node = createSettingService();
  const host = startHost(home);

  try {
    const ready = await nextMeaningful(host);
    assert.equal(ready.kind, "hostReady");

    // 1. Node 写 → Rust 读：Rust 看到 Node 写的值，且不丢 Node 的其它键。
    await node.update({ locale: "zh-CN", memoryEnabled: true } as never);
    const nodeView = await node.get();
    assert.equal(nodeView.locale, "zh-CN");
    assert.equal(nodeView.memoryEnabled, true);

    await host.send({ kind: "settingsRead", home });
    const rustRead = await nextMeaningful(host);
    assert.equal(rustRead.kind, "settingsValue");
    const rustSettings = rustRead.settings as Record<string, unknown>;
    assert.equal(rustSettings.locale, "zh-CN", "Rust 读到 Node 写的值");
    assert.equal(rustSettings.memoryEnabled, true);
    const nodeKeys = Object.keys(nodeView as Record<string, unknown>).sort();
    const rustKeys = Object.keys(rustSettings).sort();
    assert.deepEqual(rustKeys, nodeKeys, "两侧根键集合一致（同一份 schema 视图）");

    // 2. Rust 写 → Node 读：Node 的 schema 校验与视图都接受 Rust 的写入。
    await host.send({
      kind: "settingsWrite",
      home,
      patch: { locale: "en-US", memoryEnabled: false },
    });
    const rustWrite = await nextMeaningful(host);
    assert.equal(rustWrite.kind, "settingsValue");
    const nodeAfterRust = await node.get();
    assert.equal(nodeAfterRust.locale, "en-US", "Node 读到 Rust 写的值");
    assert.equal(nodeAfterRust.memoryEnabled, false);

    // 3. 交替 3 轮：每轮换值，两侧必须互相看到最新写入（不覆盖、不破坏）。
    for (let round = 0; round < 3; round += 1) {
      const locale = round % 2 === 0 ? "zh-CN" : "en-US";
      await node.update({ locale } as never);
      await host.send({ kind: "settingsRead", home });
      const read = await nextMeaningful(host);
      assert.equal(
        (read.settings as Record<string, unknown>).locale,
        locale,
        `round ${round}: Rust 必须看到 Node 刚写的值`,
      );
      await host.send({
        kind: "settingsWrite",
        home,
        patch: { memoryEnabled: round % 2 === 0 },
      });
      const write = await nextMeaningful(host);
      assert.equal(write.kind, "settingsValue");
      assert.equal(
        (await node.get()).memoryEnabled,
        round % 2 === 0,
        `round ${round}: Node 看到 Rust 写`,
      );
      assert.equal((await node.get()).locale, locale, `round ${round}: Rust 写不覆盖 Node 的键`);
    }

    // 4. 未知键保留：文件里存在、两侧 schema 都不认识的键，在对方写补丁后仍在。
    const raw = JSON.parse(await readFile(settingsFile, "utf8")) as Record<string, unknown>;
    raw["unknownFromNode"] = { kept: true };
    await writeFile(settingsFile, `${JSON.stringify(raw, null, 2)}\n`);
    await host.send({
      kind: "settingsWrite",
      home,
      patch: { locale: "zh-CN" },
    });
    const preserved = JSON.parse(await readFile(settingsFile, "utf8")) as Record<string, unknown>;
    assert.deepEqual(preserved["unknownFromNode"], { kept: true }, "Rust 写补丁不丢对方的键");
    assert.equal((await node.get()).locale, "zh-CN");

    // 5. 文件格式：2 空格缩进 JSON（TS atomicWriteText 同形）。
    const text = await readFile(settingsFile, "utf8");
    assert.ok(text.startsWith("{\n  \""), "缩进与 TS 一致");
  } finally {
    await host.stop();
  }
});
