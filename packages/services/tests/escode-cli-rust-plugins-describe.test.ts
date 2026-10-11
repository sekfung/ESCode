import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W5b：plugins/describe。已安装插件读安装目录；未安装候选
// 解析源后枚举组件（agents / commands / skills 的 frontmatter，hooks 事件名，MCP 服务名）与展示元数据。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function write(path: string, content: unknown) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(
    path,
    typeof content === "string" ? content : JSON.stringify(content),
  );
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-plugins-describe-${kind}-`));
  await mkdir(join(root, ".escode", "cli"), { recursive: true });
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [
            nodeBundle,
            "app-server",
            "--stdio",
            "--cwd",
            cwd,
          ],
          registry: true,
          mode: "yolo",
          env,
        })
      : await fixture({ root, registry: true, mode: "yolo", env });
  try {
    await configureRegistry(f);
    const storage = join(root, ".escode", "cli", "plugins");
    const market = join(storage, "marketplaces", "acme");
    const plugin = (name: string) => join(market, "plugins", name);

    const full = plugin("full");
    await write(join(full, ".escode-plugin", "plugin.json"), {
      name: "full",
      version: "1.2.3",
      author: { name: " Ada ", url: "https://ada.dev" },
      homepage: "https://full.dev",
      agents: {
        reviewer: { description: "  Reviews code  " },
        " ": {},
        blank: { description: "   " },
      },
      commands: ["./extra-commands", "../outside"],
      skills: "./more-skills",
      hooks: [
        "./hooks/hooks.json",
        "./hooks/missing.json",
        "../escape.json",
        { PreToolUse: [], Stop: [], Weird: [] },
        "./hooks/other.json",
      ],
      mcpServers: { inline: { command: "x" }, " ": { command: "y" } },
    });
    await write(
      join(full, "commands", "deploy.md"),
      "---\nname: ship\ndescription: >\n  Ship it\n  now\n\n  Second\n---\nbody",
    );
    await write(join(full, "commands", "plain.md"), "no frontmatter");
    await write(join(full, "commands", "notes.txt"), "ignored");
    await write(
      join(full, "extra-commands", "ship.md"),
      "---\nname: ship\ndescription: duplicate\n---",
    );
    await write(
      join(full, "extra-commands", "lint.md"),
      "---\ndescription: |\n  Line one\n    indented\n---",
    );
    await write(
      join(full, "agents", "helper.md"),
      "---\nname: 'helper'\ndescription: \"Helps\"\n---",
    );
    await write(join(full, "agents", "reviewer.md"), "---\nname: reviewer\n---");
    await write(
      join(full, "skills", "alpha", "SKILL.md"),
      "---\nname: alpha\ndescription: First skill\n---",
    );
    await write(join(full, "skills", "beta", "SKILL.md"), "no fm");
    await write(join(full, "skills", "node_modules", "x", "SKILL.md"), "---\nname: hidden\n---");
    await write(join(full, "skills", ".hidden", "SKILL.md"), "---\nname: hidden2\n---");
    await write(join(full, "more-skills", "SKILL.md"), "---\nname: rootskill\n---");
    await write(join(full, "more-skills", "gamma", "SKILL.md"), "---\nname: alpha\n---");
    await write(join(full, "hooks", "hooks.json"), {
      hooks: { SessionStart: [], PreToolUse: [], Bogus: [] },
    });
    await write(join(full, "hooks", "other.json"), { notHooks: true });
    await write(join(full, ".mcp.json"), {
      mcpServers: { fromFile: { command: "a" }, inline: { command: "b" } },
    });

    const loose = plugin("loose");
    await write(join(loose, "commands", "go.md"), "---\nname: go\n---");
    const bare = plugin("bare");
    await write(join(bare, "commands", "run.md"), "# run");
    await write(join(bare, ".mcp.json"), { mcpServers: { s: { command: "x" } } });
    const badname = plugin("badname");
    await write(join(badname, ".escode-plugin", "plugin.json"), {
      name: "Bad Name",
      author: "Someone",
    });
    await write(join(badname, "skills", "one", "SKILL.md"), "---\nname: one\n---");
    const inst = plugin("inst");
    await write(join(inst, ".claude-plugin", "plugin.json"), {
      name: "inst",
      version: "0.9.0",
      author: "  Bob ",
    });
    await write(join(inst, "commands", "hello.md"), "---\ndescription: Hi\n---");
    const gone = plugin("gone");
    await write(join(gone, ".escode-plugin", "plugin.json"), { name: "gone" });

    await write(join(market, "marketplace.json"), {
      name: "acme",
      plugins: [
        { name: "full", source: "./plugins/full" },
        {
          name: "loose",
          source: "./plugins/loose",
          strict: false,
          version: "3.0.0",
          homepage: "https://loose.dev",
          hooks: { Stop: [] },
        },
        { name: "bare", source: "./plugins/bare" },
        { name: "badname", source: "./plugins/badname" },
        { name: "inst", source: "./plugins/inst" },
        { name: "gone", source: "./plugins/gone" },
        { name: "nodir", source: "./plugins/nodir" },
      ],
    });

    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const rootText = JSON.stringify(root).slice(1, -1);
    const call = async (method: string, params: object) => {
      try {
        return JSON.parse(
          JSON.stringify(
            await h.client.request(method as any, { workspace, ...params }, z.any()),
          ).replaceAll(rootText, "<root>"),
        );
      } catch (error) {
        return { error: error instanceof Error ? error.message : String(error) };
      }
    };
    const observation: Record<string, unknown> = {};
    await call("plugins/install", { pluginName: "inst", marketplace: "acme" });
    await call("plugins/install", { pluginName: "gone", marketplace: "acme" });
    // 安装后改动源目录：describe 读的应是安装目录而不是源。
    await write(join(inst, "commands", "late.md"), "---\nname: late\n---");
    await rm(join(storage, "cache", "acme", "gone"), { recursive: true, force: true });
    await write(join(gone, "commands", "fallback.md"), "---\nname: fallback\n---");
    for (const name of ["full", "loose", "bare", "badname", "inst", "gone", "nodir", "ghost"]) {
      observation[name] = await call("plugins/describe", {
        pluginName: name,
        marketplace: "acme",
      });
    }
    observation.unknownMarket = await call("plugins/describe", {
      pluginName: "x",
      marketplace: "nowhere",
    });
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust describe plugins the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  // 自检：样例确实枚举出了各类组件。
  const kinds = (key: string) =>
    (node[key] as any).components.map((g: any) => g.kind);
  assert.deepEqual(kinds("full"), ["agent", "command", "skill", "hook", "mcp"]);
  assert.equal((node.full as any).metadata.author, "Ada");
  assert.ok((node.full as any).diagnostics.length > 0);
  assert.ok(
    !JSON.stringify(node.inst).includes("late"),
    "installed plugin is read from its install directory",
  );
  assert.ok(JSON.stringify(node.gone).includes("fallback"));
});
