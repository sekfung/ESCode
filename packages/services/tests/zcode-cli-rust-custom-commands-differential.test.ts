import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { zcodeWorkspacePresentationSchema } from "@zcode/shared";
import { z } from "zod";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-custom-commands.md：同一 workspace 下 Node 与 Rust 的 slash 命令目录、
// 展开后模型收到的提示词（含参数与 shell 展开）、userInput 行与标题一致；失败展开的对外表现一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));

const files: Record<string, string> = {
  "workspace/.zcode/commands/review.md":
    "---\ndescription: Review a file\nargument-hint: <file> [focus]\n---\nReview $1 focusing on $2.\nShell: !`echo shell-ok`",
  "workspace/.zcode/commands/team/notes.md": "# Team notes\nSummarize $ARGUMENTS",
  "workspace/.zcode/commands/hidden.md":
    "---\ndescription: Hidden\ndisable-noninteractive: true\n---\nhidden",
  "workspace/.zcode/commands/goal.md": "reserved name must not shadow the builtin",
  "workspace/.zcode/commands/broken.md": "Before !`exit 3` after",
  ".agents/commands/userwide.md": "User wide command",
  "plug/.zcode-plugin/plugin.json": JSON.stringify({
    name: "demo",
    commands: { gen: { content: "Generated for $ARGUMENTS", description: "Gen desc" } },
  }),
  "plug/commands/plugged.md": "---\ndescription: From plugin\n---\nPlugin body",
};

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-commands-${kind}-`));
  for (const [path, content] of Object.entries(files)) {
    await mkdir(join(root, path, ".."), { recursive: true });
    await writeFile(join(root, path), content);
  }
  await mkdir(join(root, "workspace/.zcode"), { recursive: true });
  await writeFile(
    join(root, "workspace/.zcode/config.json"),
    JSON.stringify({ plugins: { dirs: [join(root, "plug")] } }),
  );
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    if (req.stream === false || req.stream === undefined) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          id: "t",
          object: "chat.completion",
          choices: [
            { index: 0, message: { role: "assistant", content: "Title" }, finish_reason: "stop" },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      );
      return;
    }
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (text(req.messages[0] ?? {}).startsWith("Generate a concise title")) {
      event(res, { content: "Title" });
      end(res, "stop");
      return;
    }
    requests.push(req);
    event(res, { content: "done" });
    end(res, "stop");
  };
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
    await configureRegistry(f);
    const h = f.start();
    // Rust 声明动态工作流不支持（等同 TS 开关关闭），比对时去掉 Node 目录中的 workflow。
    const withoutWorkflow = (list: any[]) => list.filter((c: any) => c.name !== "workflow");
    const presentation = await h.client.request(
      "workspace/readPresentation",
      { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd } },
      zcodeWorkspacePresentationSchema.omit({ executionCapabilities: true }).strict(),
    );
    const catalog = withoutWorkflow(presentation.slashCommands ?? []);
    const turns: any[] = [];
    for (const input of [
      "/review src/a.ts 'error paths'",
      "/team:notes a b",
      "/nosuch x",
      "/init",
      "/gen x",
      "/plugged",
      "/userwide",
    ]) {
      const id = await h.create();
      await h.subscribe(`conversation/${id}`);
      const before = requests.length;
      await h.command(h.envelope("sendText", id, { text: input, mode: "yolo" }));
      await h.completed(id);
      const rows: any[] = (await h.rows(id)).rows;
      turns.push({
        input,
        model: text(requests[before]?.messages.at(-1) ?? {}).replaceAll(f.cwd, "<cwd>"),
        row: rows.find((r: any) => r.kind === "userInput")?.text,
        // 标题：Node 另有辅助模型生成标题（已知缺口），这里不比对。
      });
    }
    // shell 展开失败：记录命令 ACK 与会话终态，比对对外表现。
    const broken = await h.create();
    await h.subscribe(`conversation/${broken}`);
    const before = requests.length;
    // TS：展开失败以 failed ACK（fault.command.executionFailed + 错误文案）回复，不创建轮次。
    const { commandId: _commandId, ...ack } = (await h.command(
      h.envelope("sendText", broken, { text: "/broken", mode: "yolo" }),
    )) as any;
    const failure = { ack, rows: (await h.rows(broken)).rows.length };
    const brokenRequested = requests.length > before;
    const read = (await h.client.request("session/read", { sessionId: broken }, z.any())) as any;
    const snapshotCatalog = withoutWorkflow(
      read.snapshot?.slashCommands ?? read.slashCommands ?? [],
    );
    const observation = {
      catalog,
      snapshotCatalog,
      turns,
      failure,
      brokenRequested,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust expand custom slash commands the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检：Node 目录含自定义与插件命令，展开含 shell 输出。
  assert.ok(node.catalog.some((c: any) => c.name === "review"));
  assert.ok(node.turns[0].model.includes("shell-ok"), JSON.stringify(node.turns[0]));
  assert.deepEqual(rust.catalog, node.catalog);
  assert.deepEqual(rust.snapshotCatalog, node.snapshotCatalog);
  assert.deepEqual(rust.turns, node.turns);
  assert.equal(node.failure.ack.status, "failed");
  assert.deepEqual(rust.failure, node.failure);
  assert.equal(node.brokenRequested, false);
  assert.equal(rust.brokenRequested, false);
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
});
