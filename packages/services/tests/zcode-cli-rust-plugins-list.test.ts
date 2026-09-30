import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugins.md 第 1 期：`plugins/list` 的发现层与清单字段。
// 第 1 期不输出 components / userConfig / configuredOptions / optionSources / hookDetails /
// enabledSource / rootSource（schema 里都是可选字段），因此只比对两侧都已实现且必需的字段。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const FIELDS = [
  "id",
  "name",
  "enabled",
  "source",
  "marketplace",
  "description",
  "version",
  "author",
  "authorUrl",
  "homepage",
  "skillRootCount",
  "commandRootCount",
  "skillCount",
  "declaredMcpServerNames",
  "mcpServerNames",
] as const;

async function pluginsUnder(root: string) {
  const dirs: string[] = [];
  for (const [name, files] of Object.entries({
    alpha: {
      ".zcode-plugin/plugin.json": JSON.stringify({
        name: "alpha",
        description: "Alpha plugin",
        version: "1.2.3",
        author: { name: "Alpha Author", url: "https://example.com/alpha" },
        homepage: "https://example.com/alpha/home",
      }),
      "skills/one/SKILL.md": "---\nname: one\ndescription: First skill\n---\nbody\n",
      "skills/two/SKILL.md": "---\nname: two\n---\nbody\n",
      "commands/review.md": "---\ndescription: Review something\n---\nbody\n",
      ".mcp.json": JSON.stringify({
        mcpServers: { alpha_mcp: { type: "stdio", command: "node", args: [] } },
      }),
    },
    beta: {
      ".zcode-plugin/plugin.json": JSON.stringify({
        name: "beta",
        description: "Beta plugin",
        version: "0.1.0",
      }),
      "skills/x/SKILL.md": "---\nname: x\n---\nbody\n",
    },
  })) {
    const dir = join(root, "plugins", name);
    for (const [path, content] of Object.entries(files)) {
      await mkdir(join(dir, path, ".."), { recursive: true });
      await writeFile(join(dir, path), content);
    }
    dirs.push(dir);
  }
  return dirs;
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-${kind}-`));
  await mkdir(join(root, ".zcode", "cli"), { recursive: true });
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          mode: "yolo",
          // 官方插件：Node 从随包根发现，Rust 靠同一份随包目录 seed 进 storage（与 App 的
          // processManager 注入一致），否则 Rust 侧会缺 node-repl-host / browser-use 两行。
          env: {
            ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
            ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
            ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
          },
        })
      : await fixture({
          root,
          registry: true,
          mode: "yolo",
          env: {
            ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
            ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
            ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
          },
        });
  try {
    await configureRegistry(f);
    const dirs = await pluginsUnder(root);
    // 用户级配置：两个 inline 插件根 + 关闭 beta + 一条只声明未安装的插件（missing 行）。
    await mkdir(join(root, ".zcode", "cli"), { recursive: true });
    await writeFile(
      join(root, ".zcode", "cli", "config.json"),
      JSON.stringify({
        plugins: {
          dirs,
          enabledPlugins: { "beta@inline": false, "ghost@some-market": true },
        },
      }),
    );
    const h = f.start();
    const result = (await h.client.request(
      "plugins/list",
      { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd } },
      z.any(),
    )) as any;
    const plugins = result.plugins as any[];
    const observation = {
      plugins: plugins.map((plugin) =>
        Object.fromEntries(FIELDS.map((field) => [field, plugin[field]])),
      ),
      listedTotal: plugins.length,
      // 每个插件都必须带上 schema 必填字段（Node 与 Rust 都要能过 App 的解析）。
      missingRequired: plugins
        .flatMap((plugin) =>
          ["id", "name", "enabled", "source", "marketplace", "mcpServerNames", "rootPath"].filter(
            (field) => plugin[field] === undefined,
          ),
        )
        .sort(),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust list plugins with the same discovery fields", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
  assert.deepEqual(node.schemaErrors, []);
  // 自检：enabled / 被关闭 / 只声明未安装 三种行都在。
  const byId = Object.fromEntries(node.plugins.map((plugin: any) => [plugin.id, plugin]));
  assert.equal(byId["alpha@inline"].enabled, true);
  assert.equal(byId["alpha@inline"].description, "Alpha plugin");
  assert.equal(byId["alpha@inline"].author, "Alpha Author");
  assert.equal(byId["alpha@inline"].skillCount, 2);
  assert.equal(byId["alpha@inline"].skillRootCount, 1);
  assert.equal(byId["alpha@inline"].commandRootCount, 1);
  assert.deepEqual(byId["alpha@inline"].declaredMcpServerNames, ["alpha_mcp"]);
  assert.deepEqual(byId["alpha@inline"].mcpServerNames, ["plugin:alpha:alpha_mcp"]);
  assert.equal(byId["beta@inline"].enabled, false);
  assert.equal(byId["beta@inline"].skillCount, 0);
  assert.deepEqual(byId["beta@inline"].mcpServerNames, []);
  assert.equal(byId["ghost@some-market"].source, "missing");
  assert.deepEqual(node.missingRequired, []);
});
