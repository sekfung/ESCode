import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugins.md 第 4 期（选项面）：user / workspace 两层配置合并（dirs 并集、options 按 key）、
// plugins/list 的 enabledSource / optionSources / rootSource / configuredOptions（剔除 sensitive）/ userConfig，
// 以及 plugins/configure、plugins/resetConfig 的返回与两份配置文件落盘字节。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const FIELDS = [
  "id",
  "enabled",
  "source",
  "skillCount",
  "enabledSource",
  "optionSources",
  "rootSource",
  "configuredOptions",
  "userConfig",
] as const;

async function write(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-options-${kind}-`));
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
    const alpha = join(root, "plugins", "alpha");
    const beta = join(root, "plugins", "beta");
    await write(
      join(alpha, ".zcode-plugin", "plugin.json"),
      JSON.stringify({
        name: "alpha",
        userConfig: {
          apiKey: { type: "string", title: "API key", sensitive: true },
          region: { type: "string", title: "Region" },
        },
      }),
    );
    await write(
      join(alpha, "skills", "one", "SKILL.md"),
      "---\nname: one\n---\nb\n",
    );
    await write(
      join(beta, ".zcode-plugin", "plugin.json"),
      JSON.stringify({ name: "beta" }),
    );
    const userConfigPath = join(root, ".zcode", "cli", "config.json");
    const workspaceConfigPath = join(f.cwd, ".zcode", "config.json");
    const before = JSON.parse(
      await readFile(userConfigPath, "utf8").catch(() => "{}"),
    );
    await write(
      userConfigPath,
      JSON.stringify({
        ...before,
        plugins: {
          dirs: [alpha],
          enabledPlugins: { "alpha@inline": true },
          options: { "alpha@inline": { region: "us", apiKey: "secret" } },
        },
      }),
    );
    await write(
      workspaceConfigPath,
      JSON.stringify({
        plugins: {
          dirs: [beta],
          enabledPlugins: { "beta@inline": false },
          options: { "alpha@inline": { region: "eu" } },
        },
      }),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const scrub = (text: string) =>
      text.replaceAll(JSON.stringify(root).slice(1, -1), "<root>");
    const files = async () => ({
      user: scrub(await readFile(userConfigPath, "utf8")),
      workspace: scrub(
        await readFile(workspaceConfigPath, "utf8").catch(() => "<none>"),
      ),
    });
    const list = async (extra: Record<string, unknown> = {}) => {
      const result = (await h.client.request(
        "plugins/list",
        { workspace, ...extra },
        z.any(),
      )) as any;
      return (result.plugins as any[])
        .filter((plugin) => plugin.source === "inline")
        .map((plugin) =>
          Object.fromEntries(FIELDS.map((field) => [field, plugin[field]])),
        );
    };
    const call = async (
      method: "plugins/configure" | "plugins/resetConfig",
      params: object,
    ) => {
      try {
        return await h.client.request(
          method,
          { workspace, ...params },
          z.any(),
        );
      } catch (error) {
        return {
          error: error instanceof Error ? error.message : String(error),
        };
      }
    };
    const observation: Record<string, unknown> = {};
    observation.listMerged = await list();
    observation.listUser = await list({ configScope: "user" });
    observation.configureWorkspace = await call("plugins/configure", {
      pluginId: "alpha",
      scope: "workspace",
      options: { region: "ap", tier: 2, nested: { no: true }, flag: false },
      clearOptionKeys: [" region ", "region"],
    });
    observation.afterWorkspaceConfigure = await files();
    // 空字符串不满足协议 schema：两侧都整体拒绝（错误文案是各自校验库的措辞，只比「是否失败」）。
    const invalid = await call("plugins/configure", {
      pluginId: "alpha",
      options: {},
      clearOptionKeys: ["  "],
    });
    observation.configureInvalidRejected = "error" in (invalid as object);
    observation.configureUserDry = await call("plugins/configure", {
      pluginId: "alpha@inline",
      options: { region: "dry" },
      dryRun: true,
    });
    observation.configureUserClear = await call("plugins/configure", {
      pluginId: "alpha@inline",
      options: {},
      clearOptionKeys: ["apiKey"],
    });
    observation.afterUserConfigure = await files();
    observation.configureUnknown = await call("plugins/configure", {
      pluginId: "nope",
      options: {},
    });
    observation.listAfterConfigure = await list();
    observation.resetBetaWorkspace = await call("plugins/resetConfig", {
      pluginId: "beta@inline",
      scope: "workspace",
    });
    observation.resetAlphaUser = await call("plugins/resetConfig", {
      pluginId: "alpha@inline",
    });
    observation.resetGhostUser = await call("plugins/resetConfig", {
      pluginId: "ghost@inline",
    });
    observation.afterReset = await files();
    observation.listAfterReset = await list();
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust merge, report and write plugin options the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  // 自检：两层 dirs 并集后两个 inline 插件都在；workspace 覆盖 region、密钥不回传 UI。
  const merged = node.listMerged as any[];
  const alpha = merged.find((p) => p.id === "alpha@inline");
  const beta = merged.find((p) => p.id === "beta@inline");
  assert.ok(alpha && beta);
  assert.deepEqual(alpha.configuredOptions, { region: "eu" });
  assert.deepEqual(alpha.optionSources, {
    region: "workspace",
    apiKey: "user",
  });
  assert.equal(alpha.rootSource, "user");
  assert.equal(beta.rootSource, "workspace");
  assert.equal(beta.enabledSource, "workspace");
  assert.match(
    String((node.configureUnknown as any).error),
    /Plugin not found: nope/,
  );
});
