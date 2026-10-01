import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugins.md：发现层诊断（TS discoverNodePluginsSync 的 loader 部分）。
// 坏插件只出诊断、不拖垮整个列表；manifest 缺 version 时按 "0.0.0" 输出。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function write(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}

async function seed(root: string) {
  const dir = (name: string) => join(root, "plugins", name);
  const manifest = (name: string, kind = ".zcode-plugin") =>
    join(dir(name), kind, "plugin.json");
  await write(manifest("ok"), JSON.stringify({ name: " ok " }));
  await write(manifest("broken"), "{ not json");
  await write(manifest("array"), "[]");
  await write(manifest("bad-name"), JSON.stringify({ name: "Bad Name" }));
  await mkdir(dir("no-manifest"), { recursive: true });
  // .zcode-plugin 存在但非法时不回退到 .claude-plugin（TS findManifest 取第一个存在的文件）。
  await write(manifest("shadowed"), JSON.stringify({ name: "" }));
  await write(
    manifest("shadowed", ".claude-plugin"),
    JSON.stringify({ name: "shadowed" }),
  );
  await write(
    manifest("claude", ".claude-plugin"),
    JSON.stringify({ name: "claude", version: "2.0.0" }),
  );
  await write(manifest("dup-a"), JSON.stringify({ name: "dup" }));
  await write(manifest("dup-b"), JSON.stringify({ name: "dup" }));
  await write(
    manifest("extras"),
    JSON.stringify({ name: "extras", settings: {}, lspServers: {} }),
  );
  return [
    "ok",
    "broken",
    "array",
    "bad-name",
    "no-manifest",
    "shadowed",
    "claude",
    "dup-a",
    "dup-b",
    "extras",
    "missing-root",
  ].map(dir);
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-diag-${kind}-`));
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
    const dirs = await seed(root);
    const userConfigPath = join(root, ".zcode", "cli", "config.json");
    const before = JSON.parse(
      await readFile(userConfigPath, "utf8").catch(() => "{}"),
    );
    await write(
      userConfigPath,
      JSON.stringify({ ...before, plugins: { dirs } }),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const result = (await h.client.request(
      "plugins/list",
      { workspace },
      z.any(),
    )) as any;
    const scrub = (text: string) => text.replaceAll(root, "<root>");
    const observation = {
      plugins: (result.plugins as any[])
        .filter((plugin) => plugin.source === "inline")
        .map((plugin) => ({
          id: plugin.id,
          name: plugin.name,
          version: plugin.version,
        })),
      diagnostics: (result.diagnostics as any[]).map((item) => ({
        code: item.code,
        severity: item.severity,
        pluginId: item.pluginId,
        // JSON 语法错误的文案来自各自的解析器（V8 vs serde），只比对 code；其余文案逐字比对。
        message:
          item.code === "plugin_manifest_invalid" &&
          !/^(Invalid plugin name|Manifest must be)/.test(item.message)
            ? "<parse error>"
            : scrub(item.message),
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

test("Node and Rust report the same plugin discovery diagnostics", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust.plugins, node.plugins);
  assert.deepEqual(rust.diagnostics, node.diagnostics);
  assert.deepEqual(node.schemaErrors, []);
  // 自检。
  const ids = node.plugins.map((plugin: any) => plugin.id);
  assert.deepEqual(ids, [
    "ok@inline",
    "claude@inline",
    "dup@inline",
    "extras@inline",
  ]);
  assert.equal(node.plugins[0]?.version, "0.0.0");
  // 只看 fixture 插件产生的诊断（随包官方插件可能另有 root_not_found，两侧同样产出、已在上方比对）。
  const codes = node.diagnostics
    .filter((item: any) => !item.message.includes("zcode-plugins-official"))
    .map((item: any) => item.code)
    .sort();
  assert.deepEqual(codes, [
    "plugin_duplicate_id",
    "plugin_manifest_invalid",
    "plugin_manifest_invalid",
    "plugin_manifest_invalid",
    "plugin_manifest_invalid",
    "plugin_manifest_not_found",
    "plugin_root_not_found",
    "plugin_unsupported_component",
    "plugin_unsupported_component",
  ]);
});
