import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

process.env.ESCODE_TEST_REQUEST_MS ??= "30000";

// docs/specs/rust-plugins.md「旧 CUA 配置迁移」：Node 的 createConfig 会在装载时迁移旧 id 并
// best-effort 写回；Rust 的 plugins/list 走同一装载路径后，启用态与文件字节都必须一致。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const LEGACY = "escode-cua@zcode-plugins-official";
const CANONICAL = "computer-use@zcode-plugins-official";

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-plugins-cua-${kind}-`));
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
          env: {
            ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
            ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
            ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
          },
        })
      : await fixture({
          root,
          registry: true,
          mode: "yolo",
          env: {
            ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
            ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
            ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
          },
        });
  try {
    await configureRegistry(f);
    await mkdir(join(root, ".escode", "cli"), { recursive: true });
    const configPath = join(root, ".escode", "cli", "config.json");
    await writeFile(
      configPath,
      JSON.stringify({
        ui: { locale: "zh-CN" },
        plugins: {
          dirs: [],
          enabledPlugins: { "ghost@some-market": false, [LEGACY]: true },
          suppressedBuiltins: [LEGACY, "ghost@some-market"],
          options: { [LEGACY]: { apiKey: "legacy-key" } },
          extraKnownMarketplaces: {},
        },
        logging: { level: "warn" },
      }),
    );
    const before = await readFile(configPath, "utf8");
    const h = f.start();
    const result = (await h.client.request(
      "plugins/list",
      { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd } },
      z.any(),
    )) as any;
    const after = await readFile(configPath, "utf8");
    await h.close();
    const cua = result.plugins.find((plugin: any) => plugin.id === CANONICAL);
    return {
      before,
      after,
      canonical: cua
        ? { enabled: cua.enabled, enabledSource: cua.enabledSource }
        : null,
      hasLegacy: result.plugins.some((plugin: any) => plugin.id === LEGACY),
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust migrate legacy CUA plugin config on plugins/list", async () => {
  const node = await nodeOracle("cua-migration", () => observe("node"));
  const rust = await canonical(await observe("rust"));
  assert.deepEqual(rust, node);
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(node.canonical, { enabled: true, enabledSource: "user" });
  assert.equal(node.hasLegacy, false);
  assert.match(node.after, /"computer-use@zcode-plugins-official": true/);
  assert.doesNotMatch(node.after, /"escode-cua@zcode-plugins-official"/);
});
