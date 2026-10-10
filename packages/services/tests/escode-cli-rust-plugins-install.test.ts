import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import {
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W1b：本地源安装（市场目录内相对路径、directory 源、
// strict:false 合成 manifest、依赖闭包、默认启用不覆盖显式停用、重装保留 installedAt）与失败诊断。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function write(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}

async function tree(dir: string): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  const walk = async (current: string) => {
    let entries;
    try {
      entries = await readdir(current, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) await walk(path);
      else
        out[relative(dir, path).replaceAll("\\", "/")] = createHash("sha256")
          .update(await readFile(path))
          .digest("hex")
          .slice(0, 16);
    }
  };
  await walk(dir);
  return out;
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-plugins-install-${kind}-`));
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
    const storage = join(root, ".escode", "cli", "plugins");
    const marketDir = join(storage, "marketplaces", "acme");
    await write(
      join(marketDir, "plugins", "tool", ".escode-plugin", "plugin.json"),
      JSON.stringify({ name: "tool", version: "1.2.0", description: "Tool" }),
    );
    await write(
      join(marketDir, "plugins", "tool", "skills", "s", "SKILL.md"),
      "---\nname: s\n---\nb\n",
    );
    await write(
      join(marketDir, "plugins", "synth", "commands", "go.md"),
      "---\ndescription: go\n---\nb\n",
    );
    const helperDir = join(root, "outside", "helper");
    await write(
      join(helperDir, ".escode-plugin", "plugin.json"),
      JSON.stringify({ name: "helper" }),
    );
    await write(
      join(marketDir, "marketplace.json"),
      JSON.stringify({
        name: "acme",
        plugins: [
          {
            name: "tool",
            source: "./plugins/tool",
            dependencies: ["helper@^1.0"],
          },
          {
            name: "helper",
            version: "0.5.0",
            source: { source: "directory", path: helperDir },
          },
          {
            name: "synth",
            source: "./plugins/synth",
            strict: false,
            version: "3.0.0",
            description: "Synthesized",
            displayName: "Synth",
            category: "dev",
            commands: ["./commands/go.md"],
          },
          {
            name: "cycle-a",
            source: "./plugins/tool",
            dependencies: ["cycle-b"],
          },
          {
            name: "cycle-b",
            source: "./plugins/tool",
            dependencies: ["cycle-a"],
          },
          {
            name: "cross",
            source: "./plugins/tool",
            dependencies: ["x@other"],
          },
          { name: "needy", source: "./plugins/tool", dependencies: ["nope"] },
          { name: "badkind", source: { source: "weird" } },
        ],
      }),
    );
    const userConfigPath = join(root, ".escode", "cli", "config.json");
    const before = JSON.parse(
      await readFile(userConfigPath, "utf8").catch(() => "{}"),
    );
    await write(
      userConfigPath,
      JSON.stringify({
        ...before,
        plugins: { enabledPlugins: { "helper@acme": false } },
      }),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrubText = (text: string) =>
      text
        .replaceAll(rootText, "<root>")
        .replace(/20\d\d-\d\d-\d\dT[\d:.]+Z/g, "<time>")
        .replace(
          /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g,
          "<uuid>",
        );
    const scrub = (value: unknown) =>
      JSON.parse(scrubText(JSON.stringify(value)));
    const install = async (pluginName: string) => {
      try {
        return scrub(
          await h.client.request(
            "plugins/install",
            { workspace, pluginName, marketplace: "acme" },
            z.any(),
          ),
        );
      } catch (error) {
        return {
          error: error instanceof Error ? error.message : String(error),
        };
      }
    };
    const state = async () => ({
      installed: scrubText(
        await readFile(join(storage, "installed_plugins.json"), "utf8"),
      ),
      user: scrubText(await readFile(userConfigPath, "utf8")),
      cache: await tree(join(storage, "cache", "acme")),
    });
    const observation: Record<string, unknown> = {};
    observation.installTool = await install("tool");
    observation.afterTool = await state();
    // 重装：缓存原地替换、installedAt 保留、默认启用不再追加。
    const firstInstalledAt = JSON.parse(
      await readFile(join(storage, "installed_plugins.json"), "utf8"),
    ).plugins.find((p: any) => p.id === "tool@acme").installedAt;
    observation.reinstallTool = await install("tool");
    observation.installedAtKept =
      JSON.parse(
        await readFile(join(storage, "installed_plugins.json"), "utf8"),
      ).plugins.find((p: any) => p.id === "tool@acme").installedAt ===
      firstInstalledAt;
    observation.installSynth = await install("synth");
    observation.synthManifest = scrubText(
      await readFile(
        join(
          storage,
          "cache",
          "acme",
          "synth",
          "3.0.0",
          ".claude-plugin",
          "plugin.json",
        ),
        "utf8",
      ),
    );
    for (const name of ["cycle-a", "cross", "needy", "badkind", "ghost"]) {
      observation[`install_${name}`] = await install(name);
    }
    observation.afterAll = await state();
    observation.listed = (
      (await h.client.request("plugins/list", { workspace }, z.any())) as any
    ).plugins
      .filter((p: any) => p.marketplace === "acme")
      .map((p: any) => ({
        id: p.id,
        enabled: p.enabled,
        version: p.version,
        source: p.source,
      }));
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust install plugins from local sources the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  // 自检。
  assert.deepEqual((node.installTool as any).dependencyClosure, [
    "helper@acme",
    "tool@acme",
  ]);
  assert.equal(node.installedAtKept, true);
  const byId = Object.fromEntries((node.listed as any[]).map((p) => [p.id, p]));
  assert.equal(byId["tool@acme"].enabled, true);
  assert.equal(byId["helper@acme"].enabled, false);
  // list 显示 manifest 版本（helper 未写 → 0.0.0）；缓存目录与安装记录用条目版本 0.5.0。
  assert.equal(byId["helper@acme"].version, "0.0.0");
  assert.match((node.afterTool as any).installed, /"version": "0.5.0"/);
  for (const name of ["cycle-a", "cross", "needy", "badkind", "ghost"]) {
    assert.equal((node[`install_${name}`] as any).diagnostics.length, 1, name);
  }
});
