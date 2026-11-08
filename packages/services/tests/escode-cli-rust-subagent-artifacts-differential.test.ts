import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, readdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { escodeSessionSubagentsResultSchema } from "@escode/shared";
import { goldenOracle } from "./escode-cli-rust-golden.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { end, event, fixture } from "./escode-cli-rust-fixture.js";

// docs/specs/rust-subagents.md：同一段 Agent 调用在 Node/Rust 上都必须写
// `<storageRoot>/cli/agents/<parent>/<agent>/{output.txt,task.output,metadata.json}`，
// 正文、文件集合与 metadata 关键字段一致，且不生成 transcript.jsonl。
const pluginHostBundle = resolve("apps/escode-cli/packages/cli/dist/plugin-host.cjs");

function artifactSuffix(path: string, sid: string, agentId: string) {
  const normalized = path.replaceAll("\\", "/");
  const marker = "/cli/agents/";
  const at = normalized.indexOf(marker);
  const suffix = at < 0 ? normalized : normalized.slice(at + marker.length);
  return suffix.replace(sid, "<parent>").replace(agentId, "<agent>");
}

function iso(value: unknown) {
  return typeof value === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value)
    ? "<iso>"
    : value;
}

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `escode-subagent-artifacts-${kind}-`));
  const respond = (request: any, response: any) => {
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    const last = request.messages.at(-1);
    if (last.role === "user" && last.content === "artifact parent") {
      event(response, {
        tool_calls: [
          {
            index: 0,
            id: "launch",
            type: "function",
            function: {
              name: "Agent",
              arguments: JSON.stringify({
                description: "artifact child",
                prompt: "artifact task",
              }),
            },
          },
        ],
      });
      end(response, "tool_calls");
    } else if (last.role === "user" && last.content === "artifact task") {
      event(response, { content: "artifact result" });
      end(response, "stop");
    } else if (last.role === "tool") {
      event(response, { content: "done" });
      end(response, "stop");
    } else {
      event(response, { content: "unexpected" });
      end(response, "stop");
    }
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          registry: true,
          mode: "yolo",
          command: process.execPath,
          args: ({ cwd }) => [pluginHostBundle, "app-server", "--stdio", "--cwd", cwd],
          respond,
        })
      : await fixture({ root, registry: true, mode: "yolo", respond });
  try {
    await configureRegistry(f);
    const h = f.start();
    try {
      const sid = await h.create();
      await h.subscribe(`conversation/${sid}`);
      await h.command(h.envelope("sendText", sid, { text: "artifact parent" }));
      await h.completed(sid);
      const listing = await h.client.request(
        "session/subagents",
        { sessionId: sid },
        escodeSessionSubagentsResultSchema,
      );
      const child = listing.ended.items[0];
      assert.ok(child);
      const agentId = child.agentId;
      assert.ok(agentId);
      const agentsRoot =
        kind === "node" ? join(root, ".escode", "cli", "agents") : join(f.dataDir, "cli", "agents");
      const dir = join(agentsRoot, sid, agentId);
      const files = (await readdir(dir)).sort();
      const output = await readFile(join(dir, "output.txt"), "utf8");
      const taskOutput = await readFile(join(dir, "task.output"), "utf8");
      const metadata = JSON.parse(await readFile(join(dir, "metadata.json"), "utf8"));
      return {
        files,
        output,
        taskOutput,
        metadata: {
          agentId: metadata.agentId === agentId ? "<agent>" : "unexpected-agent",
          childSessionId:
            metadata.childSessionId === child.childSessionId ? "<child>" : "unexpected-child",
          parentSessionId: metadata.parentSessionId === sid ? "<parent>" : "unexpected-parent",
          parentToolUseId: metadata.parentToolUseId,
          profileId: metadata.profileId,
          profileName: metadata.profileSnapshot?.name,
          description: metadata.description,
          prompt: metadata.prompt,
          status: metadata.status,
          cwd: basename(metadata.cwd),
          workspaceRoot: basename(metadata.workspaceRoot),
          outputFile: artifactSuffix(metadata.outputFile, sid, agentId),
          taskOutputFile: artifactSuffix(metadata.taskOutputFile, sid, agentId),
          metadataFile: artifactSuffix(metadata.metadataFile, sid, agentId),
          createdAt: iso(metadata.createdAt),
          updatedAt: iso(metadata.updatedAt),
          completedAt: iso(metadata.completedAt),
          durationIsNumber: typeof metadata.totalDurationMs === "number",
          totalTokens: metadata.totalTokens,
          totalToolUseCount: metadata.totalToolUseCount,
        },
      };
    } finally {
      await h.close();
    }
  } finally {
    await f.close();
  }
}

test("Agent artifacts use the Node layout and metadata across Node and Rust", async () => {
  const rust = await observe("rust");
  const node = await goldenOracle("agent-artifacts", () => observe("node"));
  assert.deepEqual(rust, node);
  assert.deepEqual(rust.files, ["metadata.json", "output.txt", "task.output"]);
  assert.equal(rust.output, "artifact result");
  assert.equal(rust.taskOutput, rust.output);
  assert.equal(rust.metadata.status, "completed");
  assert.equal(rust.metadata.profileId, "general-purpose");
  assert.equal(rust.metadata.parentToolUseId, "launch");
  assert.equal(rust.metadata.outputFile, "<parent>/<agent>/output.txt");
  assert.equal(rust.metadata.taskOutputFile, "<parent>/<agent>/task.output");
  assert.equal(rust.metadata.metadataFile, "<parent>/<agent>/metadata.json");
  assert.equal(rust.metadata.createdAt, "<iso>");
  assert.equal(rust.metadata.updatedAt, "<iso>");
  assert.equal(rust.metadata.completedAt, "<iso>");
  assert.equal(rust.metadata.durationIsNumber, true);
  assert.equal(rust.metadata.totalTokens, 14);
  assert.equal(rust.metadata.totalToolUseCount, 0);
});
