import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugins.md 第 2 期：`plugins/setEnabled`。
// 断言两侧：返回值（基于写入前元数据 + enabled 覆盖 + enabledSource）、落盘配置文件的**字节**
// （保序补丁 + JSON.stringify(…, null, 2) 排版）、随后 `plugins/list` 的启用态，以及错误文案。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const FIELDS = [
  "id",
  "name",
  "enabled",
  "source",
  "marketplace",
  "skillRootCount",
  "commandRootCount",
  "skillCount",
  "declaredMcpServerNames",
  "mcpServerNames",
  "components",
  "enabledSource",
] as const;

async function pluginsUnder(root: string) {
  const dirs: string[] = [];
  for (const [name, files] of Object.entries({
    alpha: {
      ".escode-plugin/plugin.json": JSON.stringify({
        name: "alpha",
        version: "1.0.0",
      }),
      "skills/one/SKILL.md": "---\nname: one\n---\nbody\n",
    },
    beta: {
      ".escode-plugin/plugin.json": JSON.stringify({ name: "beta" }),
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

const pick = (plugin: any) =>
  Object.fromEntries(FIELDS.map((field) => [field, plugin[field]]));

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-plugins-set-${kind}-`));
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
    const dirs = await pluginsUnder(root);
    const userConfigPath = join(root, ".escode", "cli", "config.json");
    const workspaceConfigPath = join(f.cwd, ".escode", "config.json");
    // 紧凑写法 + 补丁目标之外的 key（含 `1.0` 这类数字）：用来验证保序与 JSON.stringify 排版。
    const before = JSON.parse(
      await readFile(userConfigPath, "utf8").catch(() => "{}"),
    );
    await writeFile(
      userConfigPath,
      JSON.stringify({
        ...before,
        zeta: { keep: [1, "two", null] },
        plugins: {
          dirs,
          enabledPlugins: { "beta@inline": false, "ghost@some-market": true },
        },
      }).replace('"keep":[1,', '"keep":[1.0,'),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const call = async (params: Record<string, unknown>) => {
      try {
        const result = (await h.client.request(
          "plugins/setEnabled",
          { workspace, ...params },
          z.any(),
        )) as any;
        return { enabled: result.enabled, plugin: pick(result.plugin) };
      } catch (error) {
        return {
          error: error instanceof Error ? error.message : String(error),
        };
      }
    };
    const scrub = (text: string) =>
      text.replaceAll(JSON.stringify(root).slice(1, -1), "<root>");
    const enableBeta = await call({ pluginId: "beta@inline", enabled: true });
    const userFile = scrub(await readFile(userConfigPath, "utf8"));
    const disableAlphaByName = await call({
      pluginId: " alpha ",
      enabled: false,
      scope: "workspace",
    });
    const workspaceFile = scrub(await readFile(workspaceConfigPath, "utf8"));
    const notFound = await call({ pluginId: "nope@inline", enabled: true });
    const missingRow = await call({
      pluginId: "ghost@some-market",
      enabled: false,
    });
    const listed = (await h.client.request(
      "plugins/list",
      { workspace },
      z.any(),
    )) as any;
    const observation = {
      enableBeta,
      userFile,
      disableAlphaByName,
      workspaceFile,
      notFound,
      missingRow,
      listedEnabled: Object.fromEntries(
        (listed.plugins as any[])
          .filter((plugin) => plugin.source !== "official")
          .map((plugin) => [plugin.id, plugin.enabled]),
      ),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust toggle plugins with the same result and on-disk config", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
  assert.deepEqual(node.schemaErrors, []);
  // 自检：写入前停用的 beta → 返回 enabled:true，但计数仍是写入前的 0（与 TS 相同）。
  assert.equal(node.enableBeta.enabled, true);
  assert.equal((node.enableBeta as any).plugin.skillCount, 0);
  assert.equal((node.enableBeta as any).plugin.enabledSource, "user");
  assert.equal((node.disableAlphaByName as any).plugin.id, "alpha@inline");
  assert.equal(
    (node.disableAlphaByName as any).plugin.enabledSource,
    "workspace",
  );
  assert.match(node.userFile, /"zeta": \{\n {4}"keep": \[\n {6}1,/);
  assert.match(
    node.userFile,
    /"ghost@some-market": true,\n {6}"beta@inline": true\n/,
  );
  assert.match(node.notFound.error ?? "", /Plugin not found: nope@inline/);
  // 只声明未安装的配置行不在已发现集合里，开关它与 TS 一样报未找到。
  assert.match(
    node.missingRow.error ?? "",
    /Plugin not found: ghost@some-market/,
  );
  assert.equal(node.listedEnabled["beta@inline"], true);
  assert.equal(node.listedEnabled["alpha@inline"], false);
});
