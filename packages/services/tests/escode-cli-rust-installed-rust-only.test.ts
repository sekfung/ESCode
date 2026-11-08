import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-release-rollback.md「Rust-only 安装门禁」：真实安装包只携带 Rust runtime 与
// plugin-host.cjs，必须能用包内 escode-cli-rust 创建会话并完成一轮。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const glm = process.env.ESCODE_INSTALLED_GLM;
const exe = process.platform === "win32" ? ".exe" : "";
const rust = glm ? join(glm, `escode-cli-rust${exe}`) : "";
const nodeBundle = glm ? join(glm, "zcode.cjs") : "";
const pluginHost = glm ? join(glm, "plugin-host.cjs") : "";
const app = process.env.ESCODE_INSTALLED_APP;
const execFileAsync = promisify(execFile);

test(
  "installed package runs a session with only the Rust runtime",
  { skip: !glm ? "ESCODE_INSTALLED_GLM is not set" : false },
  async () => {
    assert.ok(existsSync(rust), `installed package lacks ${rust}`);
    assert.equal(
      existsSync(nodeBundle),
      false,
      "Node runtime retired: resources/glm/zcode.cjs must not be installed",
    );
    assert.ok(
      existsSync(pluginHost),
      `Rust-only gate expects the standalone plugin host at ${pluginHost}`,
    );
    const root = await mkdtemp(join(tmpdir(), "escode-installed-rust-only-"));
    const f = await fixture({ root, binary: rust, registry: true, mode: "yolo" });
    try {
      await configureRegistry(f);
      const h = f.start();
      const sessionId = await h.create();
      await h.subscribe(`conversation/${sessionId}`);
      const after = h.messages.length;
      await h.command(
        h.envelope("sendText", sessionId, {
          text: "hello from the installed Rust-only runtime",
          mode: "yolo",
        }),
      );
      await h.completed(sessionId, after);
      assert.deepEqual(h.schemaErrors, []);
      await h.close();
    } finally {
      await f.close();
    }
  },
);

test(
  "installed package hosts a plugin without the Node agent bundle",
  { skip: !glm || !app ? "ESCODE_INSTALLED_GLM / ESCODE_INSTALLED_APP are not set" : false },
  async () => {
    const root = await mkdtemp(join(tmpdir(), "escode-installed-plugin-host-"));
    const server = join(root, "server.mjs");
    await writeFile(
      server,
      `export async function main() {
  process.stdout.write(JSON.stringify({ runtime: "plugin-host", args: process.argv.slice(2) }));
}`,
    );
    // 与 Rust runtime 注入的插件启动器一致：Electron-as-Node 执行 plugin-host.cjs，
    // 而不是依赖已被移除的 zcode.cjs。
    const { stdout } = await execFileAsync(
      app as string,
      [pluginHost, "__escode-plugin-host", server, "--probe"],
      {
        env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" },
        timeout: 30_000,
      },
    );
    assert.deepEqual(JSON.parse(stdout), {
      runtime: "plugin-host",
      args: ["--probe"],
    });
  },
);
