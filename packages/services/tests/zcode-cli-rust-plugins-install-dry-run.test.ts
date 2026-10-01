import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W5b：plugins/install dryRun 只校验不落盘——已知市场按单插件校验，
// 用户配置声明但未落盘的市场按声明源校验（id 一致、条目存在），声明与已知记录源冲突时报改指诊断。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function write(path: string, content: unknown) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, typeof content === "string" ? content : JSON.stringify(content));
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-dry-${kind}-`));
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
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          mode: "yolo",
          env,
        })
      : await fixture({ root, registry: true, mode: "yolo", env });
  try {
    await configureRegistry(f);
    const storage = join(root, ".zcode", "cli", "plugins");
    const market = join(storage, "marketplaces", "acme");
    await write(join(market, "plugins", "tool", ".zcode-plugin", "plugin.json"), {
      name: "tool",
      lspServers: {},
    });
    await write(join(market, "marketplace.json"), {
      name: "acme",
      plugins: [
        { name: "tool", source: "./plugins/tool" },
        { name: "broken", source: "./plugins/missing" },
      ],
    });
    // 已知但源与声明不同的市场（改指冲突）。
    await write(join(storage, "known_marketplaces.json"), {
      version: 1,
      marketplaces: [
        {
          id: "acme",
          source: { source: "directory", path: join(root, "elsewhere") },
          name: "acme",
          addedAt: "2026-01-01T00:00:00.000Z",
          pluginCount: 2,
        },
      ],
    });
    const declaredDir = join(root, "markets", "decl");
    await write(join(declaredDir, "marketplace.json"), {
      name: "decl",
      plugins: [{ name: "dp", source: "./dp" }],
    });
    await write(join(declaredDir, "dp", ".zcode-plugin", "plugin.json"), { name: "dp" });
    const wrongDir = join(root, "markets", "wrong");
    await write(join(wrongDir, "marketplace.json"), { name: "other", plugins: [] });
    const userConfigPath = join(root, ".zcode", "cli", "config.json");
    const before = JSON.parse(await readFile(userConfigPath, "utf8").catch(() => "{}"));
    await write(userConfigPath, {
      ...before,
      plugins: {
        extraKnownMarketplaces: {
          decl: { source: { source: "directory", path: declaredDir } },
          wrong: { source: { source: "directory", path: wrongDir } },
          acme: { source: { source: "directory", path: join(root, "conflict") } },
        },
      },
    });

    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const rootText = JSON.stringify(root).slice(1, -1);
    const dry = async (pluginName: string, marketplace: string) =>
      JSON.parse(
        JSON.stringify(
          await h.client.request(
            "plugins/install" as any,
            { workspace, pluginName, marketplace, dryRun: true },
            z.any(),
          ),
        ).replaceAll(rootText, "<root>"),
      );
    const observation: Record<string, unknown> = {};
    observation.conflict = await dry("tool", "acme");
    // 去掉冲突声明后按已知市场校验。
    const config = JSON.parse(await readFile(userConfigPath, "utf8"));
    delete config.plugins.extraKnownMarketplaces.acme;
    await write(userConfigPath, config);
    observation.known = await dry("tool", "acme");
    observation.knownBroken = await dry("broken", "acme");
    observation.knownMissing = await dry("ghost", "acme");
    observation.declared = await dry("dp", "decl");
    observation.declaredMissing = await dry("ghost", "decl");
    observation.declaredWrongId = await dry("x", "wrong");
    observation.unknownMarket = await dry("x", "nowhere");
    observation.nothingWritten = {
      installed: await readFile(join(storage, "installed_plugins.json"), "utf8").catch(
        () => null,
      ),
      markets: (await readdir(join(storage, "marketplaces"))).sort(),
      cache: await readdir(join(storage, "cache")).catch(() => []),
    };
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust validate plugin installs in dryRun the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  assert.equal((node.conflict as any).diagnostics[0].pluginId, "acme");
  assert.equal((node.known as any).diagnostics[0].code, "plugin_unsupported_component");
  assert.deepEqual((node.declared as any).diagnostics, []);
  assert.equal((node.nothingWritten as any).installed, null);
});
