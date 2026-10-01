import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugins.md 第 3 期：`plugins/overview`。
// 比对整份结果（marketplaces / availablePlugins / installedPlugins / restorableBuiltins /
// diagnostics / capability），只剔除第 4 期才实现的 hookDetails；另比对 overview 补写的
// known_marketplaces.json（除时间戳 addedAt）。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function writeJson(path: string, value: unknown) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, JSON.stringify(value));
}

async function seed(root: string) {
  const storage = join(root, ".zcode", "cli", "plugins");
  // 已安装插件：真实目录 + manifest，让发现层能加载它（componentTypes / description / version）。
  const installPath = join(storage, "cache", "acme", "tool", "1.0.0");
  await writeJson(join(installPath, ".zcode-plugin", "plugin.json"), {
    name: "tool",
    description: "Installed tool",
    version: "1.0.0",
  });
  await mkdir(join(installPath, "skills", "s"), { recursive: true });
  await writeFile(
    join(installPath, "skills", "s", "SKILL.md"),
    "---\nname: s\n---\nbody\n",
  );
  await mkdir(join(installPath, "agents"), { recursive: true });
  await writeFile(
    join(installPath, "agents", "a.md"),
    "---\nname: a\n---\nbody\n",
  );
  await writeJson(join(storage, "installed_plugins.json"), {
    version: 1,
    plugins: [
      {
        id: "tool@acme",
        name: "tool",
        marketplace: "acme",
        version: "1.0.0",
        installPath,
        installedAt: "2026-01-01T00:00:00.000Z",
        scope: "user",
      },
      {
        id: "pinned@acme",
        name: "pinned",
        marketplace: "acme",
        version: "0.0.0",
        installPath: join(storage, "cache", "acme", "pinned", "0.0.0"),
        installedAt: "2026-01-02T00:00:00.000Z",
        scope: "workspace",
        source: {
          source: "github",
          repo: "acme/pinned",
          sha: "aaaaaaaaaaaaaaaa",
        },
      },
    ],
  });
  await writeJson(join(storage, "known_marketplaces.json"), {
    version: 1,
    marketplaces: [
      {
        id: "acme",
        name: "Acme",
        source: { source: "github", repo: "acme/market" },
        description: "Acme plugins",
        lastUpdated: "2026-02-02T00:00:00.000Z",
        addedAt: "2026-01-01T00:00:00.000Z",
        pluginCount: 9,
      },
      {
        id: "broken",
        name: "broken",
        source: { source: "url", url: "https://example.invalid/m.json" },
        addedAt: "2026-01-01T00:00:00.000Z",
        pluginCount: 3,
        lastRefreshFailure: {
          code: "marketplace_refresh_failed",
          failedAt: "2026-03-03T00:00:00.000Z",
          message: "fetch failed",
        },
      },
      { id: "invalid-record", name: "x" },
    ],
  });
  await writeJson(join(storage, "marketplaces", "acme", "marketplace.json"), {
    name: "acme",
    featured: ["tool", " ", 3],
    plugins: [
      {
        name: "tool",
        description: "Tool from catalog",
        version: "1.2.0",
        skills: "./skills",
        agents: [],
        displayName: "Tool",
        description_i18n: { zh: "工具" },
        author: { name: "Acme", url: "https://acme.example" },
        examplePrompts: ["do it"],
        requiresPaidPlan: true,
      },
      {
        name: "pinned",
        source: {
          source: "github",
          repo: "acme/pinned",
          sha: "bbbbbbbbbbbbbbbbbbbb",
        },
        mcpServers: {},
      },
      { name: "  " },
      {
        name: "zipped",
        source: {
          source: "url",
          type: "zip",
          url: "https://z",
          sha256: "c".repeat(64),
        },
      },
    ],
  });
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-overview-${kind}-`));
  await mkdir(join(root, ".zcode", "cli"), { recursive: true });
  const env = {
    ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
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
    await seed(root);
    const userConfigPath = join(root, ".zcode", "cli", "config.json");
    const before = JSON.parse(
      await readFile(userConfigPath, "utf8").catch(() => "{}"),
    );
    await writeJson(userConfigPath, {
      ...before,
      plugins: {
        enabledPlugins: { "tool@acme": true },
        suppressedBuiltins: ["browser-use@zcode-plugins-official"],
        extraKnownMarketplaces: {
          "zcode-plugins-official": {
            source: { source: "github", repo: "evil/official" },
          },
          declared: {
            source: { source: "directory", path: "markets/declared" },
          },
          acme: { source: { source: "github", repo: "acme/market" } },
        },
      },
    });
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const result = (await h.client.request(
      "plugins/overview",
      { workspace },
      z.any(),
    )) as any;
    const scrub = (value: unknown) =>
      JSON.parse(
        JSON.stringify(value).replaceAll(
          JSON.stringify(root).slice(1, -1),
          "<root>",
        ),
      );
    for (const item of result.installedPlugins ?? []) delete item.hookDetails;
    const known = JSON.parse(
      await readFile(
        join(root, ".zcode", "cli", "plugins", "known_marketplaces.json"),
        "utf8",
      ),
    );
    for (const record of known.marketplaces) delete record.addedAt;
    const observation = {
      result: scrub(result),
      known: scrub(known),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust return the same plugins overview", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust.known, node.known);
  assert.deepEqual(rust.result.marketplaces, node.result.marketplaces);
  assert.deepEqual(rust.result.availablePlugins, node.result.availablePlugins);
  assert.deepEqual(rust.result.installedPlugins, node.result.installedPlugins);
  assert.deepEqual(
    rust.result.restorableBuiltins,
    node.result.restorableBuiltins,
  );
  assert.deepEqual(rust.result.diagnostics, node.result.diagnostics);
  assert.deepEqual(rust, node);
  assert.deepEqual(node.schemaErrors, []);
  // 自检：fixture 覆盖了预期的分支。
  const byId = (items: any[]) =>
    Object.fromEntries(items.map((item) => [item.id, item]));
  const markets = byId(node.result.marketplaces);
  assert.equal(markets.acme.pluginCount, 3);
  assert.deepEqual(markets.acme.featured, ["tool"]);
  assert.equal(
    markets.broken.refreshFailure.code,
    "marketplace_refresh_failed",
  );
  assert.equal(markets.declared.pluginCount, 0);
  assert.equal(markets["zcode-plugins-official"].isOfficial, true);
  const installed = byId(node.result.installedPlugins);
  assert.equal(installed["tool@acme"].updateStatus, "update-available");
  assert.equal(installed["tool@acme"].latestVersion, "1.2.0");
  assert.equal(installed["tool@acme"].enabled, true);
  assert.equal(installed["tool@acme"].listing.displayName, "Tool");
  assert.equal(installed["pinned@acme"].updateStatus, "update-available");
  assert.equal(installed["pinned@acme"].latestVersion, "bbbbbbb");
  assert.deepEqual(
    node.result.restorableBuiltins.map((item: any) => item.id),
    ["browser-use@zcode-plugins-official"],
  );
  assert.ok(
    node.result.diagnostics.some(
      (item: any) => item.code === "plugin_marketplace_declaration_reserved",
    ),
  );
});
