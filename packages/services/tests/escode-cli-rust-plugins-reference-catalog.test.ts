import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugins.md 第 3 期：`plugins/referenceCatalog(WithCategory)`。
// workspace 权威现算；session 权威冻结身份（之后 setEnabled 不影响该会话的目录），展示字段取市场 listing。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function write(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}

async function seed(root: string) {
  const storage = join(root, ".escode", "cli", "plugins");
  const inline = (name: string) => join(root, "plugins", name);
  await write(
    join(inline("alpha"), ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "alpha" }),
  );
  await write(
    join(inline("alpha"), "skills", "one", "SKILL.md"),
    "---\nname: one\n---\nb\n",
  );
  await write(
    join(inline("alpha"), "skills", "two", "SKILL.md"),
    "---\nname: two\n---\nb\n",
  );
  await write(
    join(inline("alpha"), "agents", "rev.md"),
    "---\nname: rev\n---\nb\n",
  );
  await write(
    join(inline("alpha"), ".mcp.json"),
    JSON.stringify({
      mcpServers: { m1: { type: "stdio", command: "node", args: [] } },
    }),
  );
  await write(
    join(inline("beta"), ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "beta" }),
  );
  await write(
    join(inline("dup"), ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "dup" }),
  );
  const installPath = join(storage, "cache", "acme", "dup", "1.0.0");
  await write(
    join(installPath, ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "dup" }),
  );
  await write(
    join(storage, "installed_plugins.json"),
    JSON.stringify({
      version: 1,
      plugins: [
        {
          id: "dup@acme",
          name: "dup",
          marketplace: "acme",
          version: "1.0.0",
          installPath,
          installedAt: "2026-01-01T00:00:00.000Z",
          scope: "user",
        },
      ],
    }),
  );
  await write(
    join(storage, "known_marketplaces.json"),
    JSON.stringify({
      version: 1,
      marketplaces: [
        {
          id: "acme",
          name: "acme",
          source: { source: "github", repo: "a/b" },
          pluginCount: 1,
        },
      ],
    }),
  );
  await write(
    join(storage, "marketplaces", "acme", "marketplace.json"),
    JSON.stringify({
      name: "acme",
      plugins: [
        {
          name: "dup",
          description: "  Dup plugin  ",
          icon: " https://icon ",
          displayName: "Dup",
          displayName_i18n: { zh: "重复" },
          category: "dev",
        },
      ],
    }),
  );
  return [inline("alpha"), inline("beta"), inline("dup")];
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-plugins-ref-${kind}-`));
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
    const dirs = await seed(root);
    const userConfigPath = join(root, ".escode", "cli", "config.json");
    const before = JSON.parse(
      await readFile(userConfigPath, "utf8").catch(() => "{}"),
    );
    await write(
      userConfigPath,
      JSON.stringify({
        ...before,
        plugins: {
          dirs,
          enabledPlugins: { "beta@inline": false, "dup@acme": true },
        },
      }),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const catalog = async (
      method:
        "plugins/referenceCatalog" | "plugins/referenceCatalogWithCategory",
      extra: Record<string, unknown> = {},
    ) => {
      try {
        const result = (await h.client.request(
          method,
          { workspace, ...extra },
          z.any(),
        )) as any;
        return {
          authority: result.authority,
          // 官方插件行两侧一致（plugins/list 已覆盖），这里只看 fixture 插件，避免随包内容影响断言。
          plugins: (result.plugins as any[]).filter(
            (p) => p.marketplace !== "escode-plugins-official",
          ),
        };
      } catch {
        return { error: true };
      }
    };
    const workspaceCatalog = await catalog("plugins/referenceCatalog");
    const withCategory = await catalog("plugins/referenceCatalogWithCategory");
    const sessionId = await h.create();
    const sessionBefore = await catalog("plugins/referenceCatalog", {
      sessionId,
    });
    await h.client.request(
      "plugins/setEnabled",
      { workspace, pluginId: "alpha@inline", enabled: false },
      z.any(),
    );
    const sessionAfter = await catalog("plugins/referenceCatalog", {
      sessionId,
    });
    const workspaceAfter = await catalog("plugins/referenceCatalog");
    const unknownSession = await catalog("plugins/referenceCatalog", {
      sessionId: "nope",
    });
    const observation = {
      workspaceCatalog,
      withCategory,
      sessionBefore,
      sessionAfter,
      workspaceAfter,
      unknownSession,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust build the same plugin reference catalog", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust.workspaceCatalog, node.workspaceCatalog);
  assert.deepEqual(rust.withCategory, node.withCategory);
  assert.deepEqual(rust.sessionBefore, node.sessionBefore);
  assert.deepEqual(rust.sessionAfter, node.sessionAfter);
  assert.deepEqual(rust, node);
  assert.deepEqual(node.schemaErrors, []);
  // 自检：fixture 覆盖了预期分支。
  const byId = (r: any) =>
    Object.fromEntries(r.plugins.map((p: any) => [p.pluginId, p]));
  const ws = byId(node.workspaceCatalog);
  assert.deepEqual(ws["alpha@inline"].skillQualifiedNames, [
    "alpha:one",
    "alpha:two",
  ]);
  assert.deepEqual(ws["alpha@inline"].subagentNames, ["alpha:rev"]);
  assert.deepEqual(ws["alpha@inline"].mcpServerNames, ["plugin:alpha:m1"]);
  assert.deepEqual(ws["dup@inline"].conflictingPluginIds, ["dup@acme"]);
  assert.equal(ws["dup@acme"].icon, "https://icon");
  assert.equal(ws["dup@acme"].description, "Dup plugin");
  assert.equal(ws["beta@inline"].enabled, false);
  assert.deepEqual(ws["beta@inline"].conflictingPluginIds, []);
  assert.equal(byId(node.withCategory)["dup@acme"].category, "dev");
  assert.equal(byId(node.withCategory)["alpha@inline"].category, "other");
  assert.equal(node.sessionBefore.authority, "session");
  assert.equal(byId(node.sessionAfter)["alpha@inline"].enabled, true);
  assert.equal(byId(node.workspaceAfter)["alpha@inline"].enabled, false);
  assert.deepEqual(node.unknownSession, { error: true });
});
