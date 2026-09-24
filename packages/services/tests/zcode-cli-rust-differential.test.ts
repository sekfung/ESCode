import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { z } from "zod";

const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

type Runtime = "node" | "rust";

/** 两侧共用同一个模型应答：先要一次 Read，拿到工具结果后收尾。 */
function respond(req: { messages: { role: string }[] }, res: Parameters<typeof event>[0]) {
  res.writeHead(200, { "content-type": "text/event-stream" });
  if (req.messages.at(-1)?.role === "tool") {
    event(res, { content: "done" });
    end(res, "stop");
    return;
  }
  event(res, {
    tool_calls: [
      {
        index: 0,
        id: "diff-read",
        type: "function",
        function: { name: "Read", arguments: JSON.stringify({ file_path: "sample.txt" }) },
      },
    ],
  });
  end(res, "tool_calls");
}

/** 同一场景在两侧各跑一遍，只保留语义字段（id/时间戳/路径逐次不同，不参与比较）。 */
async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-diff-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
        })
      : await fixture({ root, registry: true, respond });
  try {
    // 两侧都要有同一份 provider 配置：Node 从环境变量读，Rust 也从同一组环境键读。
    await configureRegistry(f);
    await (await import("node:fs/promises")).writeFile(join(f.cwd, "sample.txt"), "hello\n");
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "read it", mode: "yolo" }));
    await h.completed(id);
    const rows = (await h.rows(id)).rows;
    const capabilities = (await h.client.request("runtime/capabilities", {}, z.any())) as
      | Record<string, unknown>
      | undefined;
    const observation = {
      rowKinds: rows
        .map((r) => r.kind)
        .filter((k, i, all) => all.indexOf(k) === i)
        .sort(),
      toolNames: rows
        .filter((r) => r.kind === "toolCall")
        .map((r) => (r as { toolName?: string }).toolName),
      toolStatuses: rows
        .filter((r) => r.kind === "toolCall")
        .map((r) => (r as { status?: string }).status),
      capabilityKeys: capabilities ? Object.keys(capabilities).sort() : [],
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

// docs/specs/rust-release-rollback.md「功能对齐」：同一场景两侧的协议投影必须一致。
test("Node and Rust runtimes project the same rows, tools and capability keys for one turn", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 能力位是可选字段，两侧的「声明策略」不同，且已逐个确认过含义（见 rust-release-rollback.md）：
  // - Rust 显式声明自己的能力（支持哪些权限模式、能否提供 workspace 执行能力、是否有账号 provider 配置）；
  // - Node 省略这些可选位，Host 对缺席字段按「都支持」处理；
  // - plan 状态相反：Node 声明 independentPlanState，Rust 未实现 plan 故不声明，Host 因此不会给 Rust 走 plan 流程。
  // 这里把已知差异写成契约：出现新的差异键就失败。
  const rustOnly = new Set([
    "permissionModes",
    "workspaceExecutionCapabilities",
    "accountProviderConfig",
  ]);
  const nodeOnly = new Set(["independentPlanState"]);
  const extraOnRust = rust.capabilityKeys.filter((k) => !node.capabilityKeys.includes(k));
  const extraOnNode = node.capabilityKeys.filter((k) => !rust.capabilityKeys.includes(k));
  assert.deepEqual(
    extraOnRust.filter((k) => !rustOnly.has(k)),
    [],
    `Rust 新增了未记录的能力键：${extraOnRust.join(",")}`,
  );
  assert.deepEqual(
    extraOnNode.filter((k) => !nodeOnly.has(k)),
    [],
    `Node 新增了未记录的能力键：${extraOnNode.join(",")}`,
  );
  assert.deepEqual(
    node.capabilityKeys.filter((k) => rust.capabilityKeys.includes(k)),
    rust.capabilityKeys.filter((k) => node.capabilityKeys.includes(k)),
    "两侧共同声明的能力键必须一致",
  );
  assert.deepEqual(node.rowKinds, rust.rowKinds, "row kinds differ");
  assert.deepEqual(node.toolNames, rust.toolNames, "tool names differ");
  assert.deepEqual(node.toolStatuses, rust.toolStatuses, "tool statuses differ");
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
});
