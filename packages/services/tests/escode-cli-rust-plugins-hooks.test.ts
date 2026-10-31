import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-plugins.md 第 4b 期：hook 源发现、hookDetails 与发现层诊断。
// 非法 matcher 的文案含各自校验库（zod vs Rust）的措辞，只归一该条 message；其余逐字比对。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function write(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}

async function seed(root: string) {
  const dir = (name: string) => join(root, "plugins", name);
  const manifest = (name: string) => join(dir(name), ".escode-plugin", "plugin.json");
  const hook = { command: "echo ok", type: "command" };

  await write(manifest("valid"), JSON.stringify({ name: "valid" }));
  await write(
    join(dir("valid"), "hooks", "hooks.json"),
    JSON.stringify({
      hooks: {
        SessionStart: [{ hooks: [{ command: "echo start", type: "command" }] }],
        PreToolUse: [
          { matcher: "Bash", hooks: [{ args: ["h.js"], command: "node", type: "process" }] },
        ],
      },
    }),
  );
  await write(manifest("missing"), JSON.stringify({ hooks: "hooks/nope.json", name: "missing" }));
  await write(manifest("escape"), JSON.stringify({ hooks: "../outside.json", name: "escape" }));
  await write(
    manifest("duplicate"),
    JSON.stringify({ hooks: ["hooks/hooks.json", "./hooks/hooks.json"], name: "duplicate" }),
  );
  await write(
    join(dir("duplicate"), "hooks", "hooks.json"),
    JSON.stringify({ hooks: { Stop: [{ hooks: [hook] }] } }),
  );
  await write(
    manifest("invalid"),
    JSON.stringify({
      hooks: {
        Nope: [{ hooks: [hook] }],
        PreToolUse: [{ hooks: [] }],
        SessionStart: [{ hooks: [{ command: "echo inline", timeout: 3, type: "command" }] }],
        Stop: true,
      },
      name: "invalid",
    }),
  );
  await write(manifest("badjson"), JSON.stringify({ name: "badjson" }));
  await write(join(dir("badjson"), "hooks", "hooks.json"), "{ not json");

  // overview 的 installedPlugins 只对「发现到的」安装记录投影 hookDetails / componentTypes。
  const installedPath = join(root, "installed", "hooked");
  await write(
    join(installedPath, ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "hooked", version: "1.0.0" }),
  );
  await write(
    join(installedPath, "hooks", "hooks.json"),
    JSON.stringify({
      hooks: { Stop: [{ hooks: [{ command: "echo done", type: "command" }] }] },
    }),
  );
  await write(
    join(root, ".escode", "cli", "plugins", "installed_plugins.json"),
    JSON.stringify({
      plugins: [
        {
          id: "hooked@acme",
          installPath: installedPath,
          installedAt: "2026-01-01T00:00:00.000Z",
          marketplace: "acme",
          name: "hooked",
          scope: "user",
          version: "1.0.0",
        },
      ],
      version: 1,
    }),
  );

  return ["valid", "missing", "escape", "duplicate", "invalid", "badjson"].map(dir);
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-plugins-hooks-${kind}-`));
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
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          mode: "yolo",
          env,
        })
      : await fixture({ root, registry: true, mode: "yolo", env });
  try {
    await configureRegistry(f);
    const dirs = await seed(root);
    const userConfigPath = join(root, ".escode", "cli", "config.json");
    const before = JSON.parse(await readFile(userConfigPath, "utf8").catch(() => "{}"));
    await write(userConfigPath, JSON.stringify({ ...before, plugins: { dirs } }));
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const result = (await h.client.request("plugins/list", { workspace }, z.any())) as any;
    const overview = (await h.client.request("plugins/overview", { workspace }, z.any())) as any;
    const escapedRoot = JSON.stringify(root).slice(1, -1);
    const scrub = (value: unknown) =>
      JSON.parse(JSON.stringify(value).replaceAll(escapedRoot, "<root>"));
    // JSON 语法错误与非法 matcher 的文案来自各自解析器/校验库，只对齐 code/severity/位置。
    const normalize = (item: any) => {
      if (
        item.code === "plugin_hook_invalid" &&
        String(item.message).startsWith("Invalid plugin hook matcher for")
      ) {
        return { ...item, message: "<matcher error>" };
      }
      if (
        item.code === "plugin_hook_read_failed" &&
        !String(item.message).startsWith("Plugin hooks file not found:")
      ) {
        return { ...item, message: "<parse error>" };
      }
      return item;
    };
    const observation = {
      plugins: scrub(
        (result.plugins as any[])
          .filter((plugin) => plugin.source === "inline")
          .map((plugin) => ({
            components: plugin.components,
            hookDetails: plugin.hookDetails,
            id: plugin.id,
          })),
      ),
      installedPlugins: scrub(
        (overview.installedPlugins as any[])
          .filter((plugin) => plugin.id === "hooked@acme")
          .map((plugin) => ({
            componentTypes: plugin.componentTypes,
            enabled: plugin.enabled,
            hookDetails: plugin.hookDetails,
            id: plugin.id,
            scope: plugin.scope,
          })),
      ),
      diagnostics: (result.diagnostics as any[])
        // 随包官方插件的诊断随 bundle 与环境变化；本用例只比对 fixture 插件产生的 hook 诊断。
        .filter((item) => !String(item.pluginId ?? "").endsWith("@zcode-plugins-official"))
        .map((item) => ({
          code: item.code,
          message: normalize(item).message,
          pluginId: item.pluginId,
          severity: item.severity,
        })),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust discover plugin hooks identically", async () => {
  const node = await nodeOracle("observe", () => observe("node"));
  const rust = await canonical(await observe("rust"));
  assert.deepEqual(rust, node);
  assert.deepEqual(node.schemaErrors, []);

  const byId = Object.fromEntries(node.plugins.map((plugin: any) => [plugin.id, plugin]));
  assert.deepEqual(
    byId["valid@inline"].components.map((group: any) => [group.kind, group.items]),
    [["hook", [{ name: "SessionStart" }, { name: "PreToolUse" }]]],
  );
  assert.deepEqual(
    byId["valid@inline"].hookDetails.map((detail: any) => [
      detail.event,
      detail.type,
      detail.sourcePath,
    ]),
    [
      ["SessionStart", "command", "<root>/plugins/valid/hooks/hooks.json"],
      ["PreToolUse", "process", "<root>/plugins/valid/hooks/hooks.json"],
    ],
  );
  // 无效来源没有事件分组/明细；内联 manifest 的合法事件仍会产出明细（sourcePath 指向 manifest）。
  assert.deepEqual(byId["missing@inline"].hookDetails, []);
  assert.deepEqual(byId["escape@inline"].hookDetails, []);
  assert.deepEqual(byId["badjson@inline"].hookDetails, []);
  assert.deepEqual(byId["duplicate@inline"].hookDetails, [
    {
      command: "echo ok",
      event: "Stop",
      runnable: true,
      sourcePath: "<root>/plugins/duplicate/hooks/hooks.json",
      type: "command",
    },
  ]);
  assert.deepEqual(byId["invalid@inline"].hookDetails, [
    {
      command: "echo inline",
      event: "SessionStart",
      runnable: true,
      sourcePath: "<root>/plugins/invalid/.escode-plugin/plugin.json",
      timeout: 3,
      type: "command",
    },
  ]);
  assert.deepEqual(node.installedPlugins, [
    {
      componentTypes: ["hook"],
      enabled: false,
      hookDetails: [
        {
          command: "echo done",
          event: "Stop",
          runnable: true,
          sourcePath: "<root>/installed/hooked/hooks/hooks.json",
          type: "command",
        },
      ],
      id: "hooked@acme",
      scope: "user",
    },
  ]);
  assert.deepEqual(
    node.diagnostics
      .filter((item: any) => !String(item.pluginId ?? "").endsWith("@zcode-plugins-official"))
      .map((item: any) => item.code)
      .sort(),
    [
      "plugin_component_path_invalid",
      "plugin_hook_invalid",
      "plugin_hook_invalid",
      "plugin_hook_invalid",
      "plugin_hook_invalid",
      "plugin_hook_read_failed",
      "plugin_hook_read_failed",
      "plugin_hook_unsupported_event",
    ],
  );
});
