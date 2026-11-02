import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-release-rollback.md「Rust-only 安装门禁」：真实安装包在 ci-install-desktop --rust-only
// 删除 resources/glm/zcode.cjs 后，仍必须能用包内 escode-cli-rust 创建会话并完成一轮。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const glm = process.env.ESCODE_INSTALLED_GLM;
const exe = process.platform === "win32" ? ".exe" : "";
const rust = glm ? join(glm, `escode-cli-rust${exe}`) : "";
const nodeBundle = glm ? join(glm, "zcode.cjs") : "";

test(
  "installed package runs a session with only the Rust runtime",
  { skip: !glm ? "ESCODE_INSTALLED_GLM is not set" : false },
  async () => {
    assert.ok(existsSync(rust), `installed package lacks ${rust}`);
    assert.equal(
      existsSync(nodeBundle),
      false,
      "Rust-only gate expects resources/glm/zcode.cjs to be removed",
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
