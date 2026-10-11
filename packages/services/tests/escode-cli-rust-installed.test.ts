import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fixture, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-release-rollback.md「安装包真机演练」：在 CI runner 上**安装真实安装包**之后，用包里自带的两种
// runtime（Electron-as-Node 跑 resources/glm/escode.cjs、resources/glm/escode-cli-rust）在同一数据目录上走一遍
// Node → Rust → 回退 Node：
//   1. 包内 Node runtime 写出一段会话（含 Write 工具调用）；
//   2. 切到包内 Rust runtime：导入并读到该会话、跑一轮新会话；TS 源库逐字节不变；
//   3. 回退到包内 Node runtime：原会话完好、还能继续对话。
// 只在 CI 安装步骤给出路径时运行（ESCODE_INSTALLED_APP = 应用可执行文件，ESCODE_INSTALLED_GLM = resources/glm）。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const app = process.env.ESCODE_INSTALLED_APP;
const glm = process.env.ESCODE_INSTALLED_GLM;
const exe = process.platform === "win32" ? ".exe" : "";

async function digest(path: string): Promise<string> {
  return createHash("sha256").update(await readFile(path)).digest("hex");
}

const nodeOptions = (root: string) => ({
  root,
  command: app!,
  args: ({ cwd }: { cwd: string }) => [join(glm!, "escode.cjs"), "app-server", "--stdio", "--cwd", cwd],
  registry: true,
  mode: "yolo" as const,
  // 与桌面 Host 一致：用应用自带的 Electron 作为 Node 运行 agent bundle。
  env: { ELECTRON_RUN_AS_NODE: "1" },
});

async function turn(h: Harness, sessionId: string, text: string) {
  await h.subscribe(`conversation/${sessionId}`);
  const after = h.messages.length;
  await h.command(h.envelope("sendText", sessionId, { text, mode: "yolo" }));
  await h.completed(sessionId, after);
}

test(
  "installed package switches Node → Rust → Node on the same data without loss",
  { skip: !app || !glm ? "ESCODE_INSTALLED_APP / ESCODE_INSTALLED_GLM not set" : false },
  async () => {
    const rust = join(glm!, `escode-cli-rust${exe}`);
    assert.ok(existsSync(join(glm!, "escode.cjs")), "installed package lacks resources/glm/escode.cjs");
    assert.ok(existsSync(rust), "installed package lacks the bundled Rust runtime");
    const root = await mkdtemp(join(tmpdir(), "escode-installed-"));

    // 1) 包内 Node runtime。
    const first = await fixture(nodeOptions(root));
    let sessionId = "";
    try {
      await configureRegistry(first);
      const h = first.start();
      sessionId = await h.create();
      await turn(h, sessionId, "write");
      await h.close();
      assert.deepEqual(h.schemaErrors, []);
    } finally {
      await first.close();
    }
    const tsDb = join(root, "ts.sqlite");
    const before = await digest(tsDb);

    // 2) 切到包内 Rust runtime：导入 TS 数据、读到原会话、跑一轮新会话。
    const second = await fixture({ root, binary: rust, legacy: true, registry: true, mode: "yolo" });
    try {
      await configureRegistry(second);
      const h = second.start();
      const rows = (await h.rows(sessionId)).rows;
      assert.ok(rows.some((r: any) => r.kind === "userInput" && r.text === "write"), "Rust lost the Node session input");
      assert.ok(rows.some((r: any) => r.kind === "toolCall" && r.toolName === "Write"), "Rust lost the Node tool call");
      const rustSession = await h.create();
      await turn(h, rustSession, "hello");
      await h.close();
      assert.deepEqual(h.schemaErrors, []);
    } finally {
      await second.close();
    }
    assert.equal(await digest(tsDb), before, "Rust runtime modified the TS store");

    // 3) 回退到包内 Node runtime：原会话完好且可继续。
    const third = await fixture(nodeOptions(root));
    try {
      await configureRegistry(third);
      const h = third.start();
      const rows = (await h.rows(sessionId)).rows;
      assert.ok(rows.some((r: any) => r.kind === "userInput" && r.text === "write"), "rollback lost the session");
      await turn(h, sessionId, "after rollback");
      await h.close();
      assert.deepEqual(h.schemaErrors, []);
    } finally {
      await third.close();
    }
  },
);
