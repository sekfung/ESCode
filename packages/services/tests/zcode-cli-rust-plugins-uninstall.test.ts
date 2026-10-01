import assert from "node:assert/strict";
import test from "node:test";
import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W1a：plugins/uninstall（市场安装 / 内置 / 不存在）与
// plugins/restoreBuiltin。比对协议返回、installed_plugins.json 与用户配置的字节、缓存与数据目录是否保留。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const BUILTIN = "browser-use@zcode-plugins-official";

async function write(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}
const exists = (path: string) =>
  access(path).then(
    () => true,
    () => false,
  );

async function observe(kind: Runtime) {
  const root = await mkdtemp(
    join(tmpdir(), `zcode-plugins-uninstall-${kind}-`),
  );
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
    const storage = join(root, ".zcode", "cli", "plugins");
    const installPath = join(storage, "cache", "acme", "tool", "1.0.0");
    await write(
      join(installPath, ".zcode-plugin", "plugin.json"),
      JSON.stringify({ name: "tool", version: "1.0.0" }),
    );
    await write(join(storage, "data", "tool@acme", "state.json"), "{}");
    await write(join(storage, "data", BUILTIN, "state.json"), "{}");
    await write(
      join(storage, "installed_plugins.json"),
      JSON.stringify({
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
            id: "keep@acme",
            name: "keep",
            marketplace: "acme",
            version: "2.0.0",
            installPath: join(storage, "cache", "acme", "keep", "2.0.0"),
            installedAt: "2026-01-02T00:00:00.000Z",
            scope: "user",
            extra: { kept: true },
          },
        ],
      }),
    );
    const userConfigPath = join(root, ".zcode", "cli", "config.json");
    const before = JSON.parse(
      await readFile(userConfigPath, "utf8").catch(() => "{}"),
    );
    await write(
      userConfigPath,
      JSON.stringify({
        ...before,
        plugins: {
          enabledPlugins: { "tool@acme": true, [BUILTIN]: true },
          options: { "tool@acme": { k: 1 } },
          suppressedBuiltins: ["tool@acme"],
        },
      }),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const scrub = (value: unknown) =>
      JSON.parse(
        JSON.stringify(value)
          .replaceAll(JSON.stringify(root).slice(1, -1), "<root>")
          .replace(/"installedAt":"20\d\d-[^"]*"/g, (match) =>
            match.includes("2026-01-0") ? match : '"installedAt":"<now>"',
          ),
      );
    const call = async (
      method: "plugins/uninstall" | "plugins/restoreBuiltin",
      params: object,
    ) => {
      try {
        return scrub(
          await h.client.request(method, { workspace, ...params }, z.any()),
        );
      } catch (error) {
        return {
          error: error instanceof Error ? error.message : String(error),
        };
      }
    };
    // 文件原文里的路径是一次 JSON 转义；再经 scrub 的 JSON.stringify 会变成两次，先按原文替换。
    const raw = (text: string) =>
      text.replaceAll(JSON.stringify(root).slice(1, -1), "<root>");
    const files = async () => ({
      installed: scrub(
        raw(await readFile(join(storage, "installed_plugins.json"), "utf8")),
      ),
      user: scrub(raw(await readFile(userConfigPath, "utf8"))),
      toolCache: await exists(installPath),
      toolData: await exists(join(storage, "data", "tool@acme")),
      builtinData: await exists(join(storage, "data", BUILTIN)),
    });
    const listed = async () =>
      (
        (await h.client.request("plugins/list", { workspace }, z.any())) as any
      ).plugins
        .map((plugin: any) => plugin.id)
        .sort();
    const observation: Record<string, unknown> = {};
    observation.uninstallInstalled = await call("plugins/uninstall", {
      pluginId: "tool@acme",
    });
    observation.afterInstalled = await files();
    observation.uninstallBuiltin = await call("plugins/uninstall", {
      pluginName: "browser-use",
      marketplace: "zcode-plugins-official",
    });
    observation.afterBuiltin = await files();
    observation.builtinCacheKept = await exists(
      join(storage, "cache", "zcode-plugins-official", "browser-use"),
    );
    observation.listAfterUninstall = await listed();
    observation.uninstallUnknown = await call("plugins/uninstall", {
      pluginId: "ghost@acme",
    });
    observation.uninstallMissingSelector =
      "error" in (await call("plugins/uninstall", {}));
    observation.restore = await call("plugins/restoreBuiltin", {
      pluginId: BUILTIN,
    });
    observation.afterRestore = await files();
    observation.listAfterRestore = await listed();
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust uninstall and restore plugins the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  // 自检。
  assert.equal((node.uninstallInstalled as any).removedPlugin.id, "tool@acme");
  assert.equal((node.afterInstalled as any).toolCache, false);
  assert.equal((node.afterInstalled as any).toolData, false);
  assert.equal((node.uninstallBuiltin as any).removedPlugin.id, BUILTIN);
  assert.equal((node.afterBuiltin as any).builtinData, false);
  assert.equal(node.builtinCacheKept, true);
  assert.ok(!(node.listAfterUninstall as string[]).includes(BUILTIN));
  assert.ok((node.listAfterRestore as string[]).includes(BUILTIN));
  assert.equal(node.uninstallMissingSelector, true);
});
