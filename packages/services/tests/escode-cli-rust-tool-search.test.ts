// Node ↔ Rust ToolSearch 实时差分：Node 侧跑上游 `search.ts` oracle，Rust 侧跑
// `examples/tool_search_fixture`。本 fork 的 Node CLI 没有合并 ToolSearch，因此不启动 Node CLI。
import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { once } from "node:events";
import { resolve } from "node:path";
import { searchTools, type ToolSearchCandidate } from "./tool-search-oracle.js";

type Candidate = {
  name: string;
  aliases?: string[];
  mcp?: { serverName: string; toolName: string };
  description?: string;
  searchHint?: string;
  deferred: boolean;
};

const builtins: Candidate[] = [
  { name: "Read", description: "Read a file from disk", deferred: false },
  { name: "Write", description: "Write a file", deferred: false },
  { name: "Edit", description: "Edit an existing file", deferred: false },
  { name: "Bash", description: "Run a shell command", deferred: false },
  { name: "Agent", description: "Launch a subagent", deferred: false },
  { name: "SendMessage", description: "Message a subagent", deferred: false },
  { name: "ReadSessionContext", description: "Read another session context", deferred: false },
  { name: "CronCreate", description: "Create an automation", deferred: false },
  { name: "CreateWorkflow", description: "Create a dynamic workflow run", deferred: false },
  { name: "ListModels", description: "List available models", deferred: false },
  { name: "WebSearch", description: "Search the web", deferred: false },
];
const mcpSpecs: Array<[name: string, description: string, searchHint: string]> = [
  ["mcp__github__create_issue", "Create a GitHub issue", "github issues bug tracker"],
  ["mcp__github__list_issues", "List GitHub issues", "github issues query"],
  ["mcp__github__create_pull_request", "Create a pull request", "github pr review"],
  ["mcp__slack__send_message", "Send a Slack message", "slack chat notify"],
  ["mcp__linear__create_issue", "Create a Linear issue", "linear tracker ticket"],
  ["mcp__notion__search_pages", "Search Notion pages", "notion docs wiki"],
];
const mcp: Candidate[] = mcpSpecs.map(([name, description, searchHint]) => ({
  name,
  description,
  searchHint,
  mcp: {
    serverName: name.split("__")[1]!,
    toolName: name.split("__")[2]!,
  },
  deferred: true,
}));
const catalog: Candidate[] = [
  ...builtins,
  { name: "Task", aliases: ["AgentAlias"], description: "Alias for Agent", deferred: true },
  ...mcp,
];

const cases: [string, number][] = [
  ["select:CronCreate", 5],
  ["select:Task", 5],
  ["select:AgentAlias", 5],
  ["select:create_issue", 5],
  ["select:mcp__slack__send_message,Unknown", 5],
  ["read", 5],
  ["session context", 5],
  ["cron create", 5],
  ["workflow run", 5],
  ["list models", 5],
  ["create issue", 5],
  ["+slack send", 5],
  ["pull request", 5],
  ["mcp__github", 5],
  ["zzz", 5],
  ["plan mode", 5],
  ["web search", 5],
  ["+notion page", 5],
  ["issue", 2],
];

test("Rust ToolSearch matches the Node oracle", async () => {
  const binary = resolve(
    `apps/escode-cli-rust/target/debug/examples/tool_search_fixture${
      process.platform === "win32" ? ".exe" : ""
    }`,
  );
  const child = spawn(binary, [], { stdio: ["pipe", "pipe", "inherit"] });
  const lines = createInterface({ input: child.stdout });
  try {
    for (const [query, maxResults] of cases) {
      const next = once(lines, "line");
      child.stdin.write(`${JSON.stringify({ query, maxResults, catalog })}\n`);
      const [line] = await next;
      const result = JSON.parse(line);
      assert.deepEqual(
        result.matches,
        searchTools(query, catalog as ToolSearchCandidate[], maxResults),
        `query=${query} maxResults=${maxResults}`,
      );
    }
  } finally {
    child.stdin.end();
    lines.close();
  }
  assert.deepEqual(await once(child, "close"), [0, null]);
});
