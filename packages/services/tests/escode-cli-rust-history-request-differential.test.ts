import assert from "node:assert/strict";
import test from "node:test";
import { copyFile, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";
import { copyLegacySeed } from "./escode-cli-rust-legacy-seed.js";
import type { SessionId } from "@escode/contracts";

// 模型请求语义：同一份由真实 TS store 写出的历史，Node 与 Rust 继续对话时发给模型的会话部分必须一致。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const session = "history-request" as SessionId;
const workspace = "history-request-workspace";

/** 只保留会话部分：去掉 system 与 system-reminder（两侧提示词/提醒的差异由其它差分覆盖）。 */
function conversation(messages: any[]) {
  return messages
    .filter((m) => m.role !== "system")
    .filter(
      (m) =>
        !(
          m.role === "user" &&
          (typeof m.content === "string" ? m.content : JSON.stringify(m.content)).startsWith(
            "<system-reminder>",
          )
        ),
    )
    .map((m) => ({
      role: m.role,
      content:
        typeof m.content === "string"
          ? m.content
          : Array.isArray(m.content)
            ? m.content.map((p: any) => p.text ?? p.type).join("")
            : (m.content ?? ""),
      tool_calls: (m.tool_calls ?? []).map((c: any) => [
        c.id,
        c.function?.name,
        JSON.parse(c.function?.arguments || "{}"),
      ]),
      tool_call_id: m.tool_call_id ?? null,
    }));
}

async function observe(kind: "node" | "rust", source: string) {
  const root = await mkdtemp(join(tmpdir(), `escode-hreq-${kind}-`));
  await copyFile(source, join(root, "ts.sqlite"));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ root, registry: true, legacy: true });
  try {
    await configureRegistry(f);
    const h = f.start(workspace);
    await h.subscribe(`conversation/${session}`);
    await h.command(
      h.envelope("sendText", session, {
        text: "next question",
        mode: "yolo",
        modelSelection: {
          providerId: "personal:fixture",
          modelId: "model-a",
          options: { reasoningLevel: "low" },
        },
      }),
    );
    await h.completed(session);
    const request = f.requests[0]!;
    const observation = {
      conversation: conversation(request.messages),
      tools: (request.tools ?? []).map((t: any) => t.function ?? t),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust send the same conversation to the model when continuing an imported history", async () => {
  const dir = await mkdtemp(join(tmpdir(), "escode-hreq-seed-"));
  const source = join(dir, "ts.sqlite");
  await copyLegacySeed("history-request-differential.legacy", source, {
    workspace,
    root: dir,
  });
  const node = await nodeOracle("imported-history", () => observe("node", source));
  const rust = canonical(await observe("rust", source));
  if (process.env.ESCODE_HREQ_DUMP)
    console.log(
      "DUMP",
      JSON.stringify({
        node: node.conversation,
        rust: rust.conversation,
        nodeTools: node.tools,
        rustTools: rust.tools,
      }),
    );
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
  // 自检：历史确实进入了请求（6 轮问答 + 本轮问题）。
  assert(node.conversation.some((m) => m.content === "question 0"));
  assert.deepEqual(rust.conversation, node.conversation);
  // 模型可见工具面（docs/specs/rust-tool-surface.md）：名称、顺序、描述逐字一致，参数 schema 语义一致。
  // 尚未实现的工具显式列出，实现后移出。
  const pending = new Set<string>();
  // 局部排序函数与 golden 的 canonical 无关：只用于忽略参数 schema 的对象键顺序。
  const sortKeys = (value: any): any =>
    Array.isArray(value)
      ? value.map(sortKeys)
      : value && typeof value === "object"
        ? Object.fromEntries(
            Object.keys(value)
              .sort()
              .map((key) => [key, sortKeys(value[key])]),
          )
        : value;
  const surface = (tools: any[]) =>
    tools
      .filter((tool) => !pending.has(tool.name))
      .map((tool) => ({
        name: tool.name,
        description: tool.description,
        parameters: sortKeys(tool.parameters),
      }));
  assert.deepEqual(surface(rust.tools), surface(node.tools));
});
