import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdir, mkdtemp, readFile, readdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-project-memory.md：Host 开启 Memory 时，Node 与 Rust 的 Memory 段、MEMORY.md 索引注入、
// 轮次后的提取请求与写入的记忆文件一致；主会话直接写记忆 Markdown 在 build 模式下无需确认且跳过提取。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));
const EXTRACTION = "You are now acting as the memory extraction subagent";

function reply(req: any, res: any, message: { content?: string; calls?: any[] }) {
  const calls = (message.calls ?? []).map((c, index) => ({
    index,
    id: c.id,
    type: "function",
    function: { name: c.name, arguments: JSON.stringify(c.input) },
  }));
  if (req.stream === false || req.stream === undefined) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        id: "x",
        object: "chat.completion",
        choices: [
          {
            index: 0,
            message: {
              role: "assistant",
              content: message.content ?? "",
              ...(calls.length ? { tool_calls: calls.map(({ index: _i, ...c }) => c) } : {}),
            },
            finish_reason: calls.length ? "tool_calls" : "stop",
          },
        ],
        usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
      }),
    );
    return;
  }
  res.writeHead(200, { "content-type": "text/event-stream" });
  event(res, calls.length ? { tool_calls: calls } : { content: message.content ?? "" });
  end(res, calls.length ? "tool_calls" : "stop");
}
const memoryRootOf = (req: any) =>
  /persistent file-based memory at `([^`]+)\/`/.exec(
    req.messages
      .filter((m: any) => m.role === "system")
      .map(text)
      .join("\n"),
  )?.[1];

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-memory-${kind}-`));
  // 关闭插件：官方插件 seed 环境两侧不同（技能提醒、MCP 工具），与记忆无关的差异不进入比对。
  await mkdir(join(root, "workspace", ".zcode"), { recursive: true });
  await writeFile(
    join(root, "workspace", ".zcode", "config.json"),
    JSON.stringify({ plugins: { enabled: false } }),
  );
  const main: any[] = [];
  const extraction: any[] = [];
  let extractionStep = 0;
  const respond = (req: any, res: any) => {
    const last = text(req.messages.at(-1) ?? {});
    if (text(req.messages[0] ?? {}).startsWith("Generate a concise title")) {
      return reply(req, res, { content: "Title" });
    }
    const memoryRoot = memoryRootOf(req);
    const isExtraction = req.messages.some(
      (m: any) => m.role === "user" && text(m).startsWith(EXTRACTION),
    );
    if (isExtraction) {
      extraction.push(req);
      if (extractionStep++ === 0) {
        return reply(req, res, {
          calls: [
            {
              id: "x1",
              name: "Write",
              input: {
                file_path: `${memoryRoot}/prefs.md`,
                content:
                  "---\nname: prefs\ndescription: Prefers tabs\nmetadata:\n  type: user\n---\nThe user prefers tabs.",
              },
            },
            {
              id: "x2",
              name: "Write",
              input: {
                file_path: `${memoryRoot}/MEMORY.md`,
                content: "- [Prefs](prefs.md) — tabs",
              },
            },
          ],
        });
      }
      return reply(req, res, { content: "Done." });
    }
    main.push(req);
    if (last.includes("write it down yourself") && req.messages.at(-1)?.role === "user") {
      return reply(req, res, {
        calls: [
          {
            id: "d1",
            name: "Write",
            input: { file_path: `${memoryRoot}/direct.md`, content: "direct memory" },
          },
        ],
      });
    }
    return reply(req, res, { content: "ok" });
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "build",
        })
      : await fixture({ root, registry: true, respond, mode: "build" });
  try {
    await configureRegistry(f);
    const h = f.start();
    h.memoryEnabled = true;
    const first = await h.create();
    await h.subscribe(`conversation/${first}`);
    await h.command(h.envelope("sendText", first, { text: "please remember that I prefer tabs" }));
    await h.completed(first);
    const memoryRoot = memoryRootOf(main[0]);
    assert.ok(memoryRoot, `${kind} injects the Memory section`);
    // 提取在轮次后异步执行，等待两个文件落盘。
    for (let i = 0; i < 100; i++) {
      const names = await readdir(memoryRoot!).catch((): string[] => []);
      if (names.includes("prefs.md") && names.includes("MEMORY.md")) break;
      await delay(100);
    }
    await delay(300);
    const extractionsAfterFirst = extraction.length;
    const second = await h.create();
    await h.subscribe(`conversation/${second}`);
    const beforeSecond = main.length;
    await h.command(h.envelope("sendText", second, { text: "now write it down yourself" }));
    await h.completed(second);
    await delay(1500);
    const rows = (await h.rows(second)).rows as any[];
    const normalize = (value: string) => value.replaceAll(memoryRoot!, "<memory>");
    const files: Record<string, string> = {};
    for (const name of (await readdir(memoryRoot!)).sort()) {
      files[name] = (await readFile(join(memoryRoot!, name), "utf8")).replaceAll(
        first,
        "<session>",
      );
    }
    // TS resolveProjectMemoryRoot：无 identity 时以绝对 workspace 路径（Windows 小写）哈希。
    const key = process.platform === "win32" ? f.cwd.toLowerCase() : f.cwd;
    const hash = createHash("sha256").update(key).digest("hex").slice(0, 16);
    const expectedRoot = join(
      root,
      ".zcode",
      "cli",
      "memories",
      "projects",
      `workspace-${hash}`,
      "memory",
    );
    const observation = {
      memoryRootMatches: memoryRoot === expectedRoot || `${memoryRoot} != ${expectedRoot}`,
      // 只比对 Memory 段本身：其余系统段由 prompt 差分覆盖，官方插件 seed 环境不同会影响 guidance 段。
      system: normalize(
        /# Memory\n[\s\S]*?(?=\n\n# Environment)/.exec(
          main[0].messages
            .filter((m: any) => m.role === "system")
            .map(text)
            .join("\n"),
        )?.[0] ?? "",
      ),
      secondContext: normalize(
        main[beforeSecond].messages
          .filter((m: any) => m.role === "user")
          .map(text)
          .find((t: string) => t.includes("# agentsMd")) ?? "",
      ),
      extractionPrompt: normalize(text(extraction[0]?.messages.at(-1) ?? {})),
      // 官方插件 MCP 工具取决于 seed 环境（另有用例覆盖），只比对内置工具面。
      extractionTools: (extraction[0]?.tools ?? [])
        .map((t: any) => t.function?.name ?? t.name)
        .filter((name: string) => !name.startsWith("mcp__")),
      extractionRoles: extraction[0]?.messages.map((m: any) => m.role),
      extractionCount: extractionsAfterFirst,
      extractionsAfterSecond: extraction.length - extractionsAfterFirst,
      files,
      pendingApproval: rows.some((r) => r.status === "pendingApproval"),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust inject, extract and permit project memory the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检：Node 注入了索引并完成两轮提取，直接写入的记忆文件落盘且未弹确认。
  assert.ok(node.secondContext.includes("- [Prefs](prefs.md) — tabs"), node.secondContext);
  assert.equal(node.extractionCount, 2);
  assert.ok(node.files["direct.md"]);
  assert.equal(node.pendingApproval, false);

  assert.equal(node.memoryRootMatches, true);
  assert.equal(rust.memoryRootMatches, true);
  assert.equal(rust.system, node.system);
  assert.equal(rust.secondContext, node.secondContext);
  assert.equal(rust.extractionPrompt, node.extractionPrompt);
  assert.deepEqual(rust.extractionTools, node.extractionTools);
  assert.deepEqual(rust.extractionRoles, node.extractionRoles);
  assert.equal(rust.extractionCount, node.extractionCount);
  assert.equal(rust.extractionsAfterSecond, node.extractionsAfterSecond);
  assert.deepEqual(rust.files, node.files);
  assert.equal(rust.pendingApproval, false);
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
});
