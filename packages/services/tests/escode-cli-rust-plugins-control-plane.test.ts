import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { z } from "zod";
import { binary, fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugins.md「插件控制面的 workspace 语义」：Host 的插件管理进程以内部目录
// （~/.escode/plugin-workspace）为 cwd，真实 workspace 经协议参数传入。Rust 不能拿参数和自身 cwd 比较，
// 更不能因请求级校验失败退出整个进程（Host 会把它误判为“Rust 就绪前失败”并永久回退 Node）。

test("plugins/* accept a protocol workspace that differs from the runtime cwd", async () => {
  const f = await fixture({ registry: true, mode: "yolo" });
  try {
    await configureRegistry(f);
    const realWorkspace = join(f.root, "real-project");
    await mkdir(realWorkspace, { recursive: true });
    const h = f.start();
    const workspace = { workspace: { workspacePath: realWorkspace, workspaceKey: realWorkspace } };
    const list = (await h.client.request("plugins/list", workspace, z.any())) as {
      plugins: unknown[];
    };
    assert.ok(Array.isArray(list.plugins), "plugins/list answers for the protocol workspace");
    const overview = (await h.client.request("plugins/overview", workspace, z.any())) as Record<
      string,
      unknown
    >;
    assert.equal(typeof overview, "object", "plugins/overview answers for the protocol workspace");
    assert.equal(h.child.exitCode, null, "the runtime keeps serving");
    await h.close();
  } finally {
    await f.close();
  }
});

test("request-level validation errors become RPC errors instead of ending the runtime", async () => {
  const f = await fixture({ registry: true, mode: "yolo" });
  try {
    await configureRegistry(f);
    const h = f.start();
    // 缺少必填参数：必须回 RPC 错误，进程继续服务后续请求。
    await assert.rejects(
      h.client.request("v4/conversation/usage", {}, z.any()),
      /sessionId is required/,
    );
    await assert.rejects(
      h.client.request("v4/conversation/backgroundBashOutput", { sessionId: "sess_x" }, z.any()),
      /workId is required/,
    );
    const list = (await h.client.request(
      "plugins/list",
      { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd } },
      z.any(),
    )) as { plugins: unknown[] };
    assert.ok(Array.isArray(list.plugins), "later requests are still served");
    assert.equal(h.child.exitCode, null);
    await h.close();
  } finally {
    await f.close();
  }
});

test("a second runtime on an owned workspace exits with the dedicated ownership code", async () => {
  const f = await fixture({ registry: true, mode: "yolo" });
  try {
    await configureRegistry(f);
    const owner = f.start();
    // 等首个 runtime 拿到 owner 锁并完成存储启动后再拉第二个。
    await owner.client.request(
      "plugins/list",
      { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd } },
      z.any(),
    );
    const second = spawn(
      binary,
      ["app-server", "--stdio", "--cwd", f.cwd, "--data-dir", f.dataDir],
      {
        stdio: ["pipe", "ignore", "pipe"],
        env: {
          ...process.env,
          HOME: f.root,
          USERPROFILE: f.root,
          ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: join(f.root, "builtin.json"),
          ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: join(f.root, "personal.json"),
        },
      },
    );
    const [code] = (await once(second, "exit")) as [number | null];
    // Host 契约：75 表示所有权冲突（可等待旧 owner 退出后重试），不是 Rust runtime 故障。
    assert.equal(code, 75);
    assert.equal(owner.child.exitCode, null, "the owner keeps serving");
    await owner.close();
  } finally {
    await f.close();
  }
});
