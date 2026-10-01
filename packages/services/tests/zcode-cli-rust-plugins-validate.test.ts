import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W5b：plugins/validate。只读校验：已知市场里的单个插件
// （manifest / 依赖 / MCP 声明 / 兼容性）与按 source 加载的市场（形状、远端条目延后、本地条目深扫）。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function write(path: string, content: unknown) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(
    path,
    typeof content === "string" ? content : JSON.stringify(content),
  );
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-validate-${kind}-`));
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
    const market = join(storage, "marketplaces", "acme");
    const plugin = (name: string) => join(market, "plugins", name);
    await write(join(plugin("good"), ".zcode-plugin", "plugin.json"), {
      name: "good",
      version: "1.0.0",
      mcpServers: "./servers.json",
    });
    await write(join(plugin("good"), "servers.json"), {
      mcpServers: { ok: { command: "node", args: ["${CLAUDE_PLUGIN_ROOT}/a.js"] } },
    });
    await write(join(plugin("risky"), ".claude-plugin", "plugin.json"), {
      name: "risky",
      lspServers: {},
      settings: {},
      userConfig: {
        token: { type: "string", required: true, sensitive: true },
        region: { type: "string", default: "us" },
      },
      mcpServers: [
        "./bundle.mcpb",
        {
          ws: { type: "ws", url: "ws://x" },
          noUrl: { type: "sse" },
          envMissing: { command: "run", env: { KEY: "${API_KEY}" } },
          sessionArg: { command: "run", args: ["${ZCODE_SESSION_ID}"] },
          sensitiveArg: { command: "run", args: ["${user_config.token}"] },
          regionOk: { url: "https://x/${user_config.region}" },
          badOauth: { url: "https://x", oauth: { type: "magic" } },
          badAuth: { url: "https://x", auth: { type: "other" } },
          officialSse: {
            type: "sse",
            url: "https://x",
            auth: { type: "zcode_official", provider: "jwt_token" },
          },
          reserved: {
            url: "https://x",
            headers: { Authorization: "a" },
            auth: { type: "zcode_official", provider: "jwt_token" },
          },
        },
      ],
    });
    await write(join(plugin("risky"), ".mcp.json"), "[1]");
    await write(join(plugin("escape"), ".zcode-plugin", "plugin.json"), {
      name: "escape",
      mcpServers: "../../outside.json",
    });
    await write(join(plugin("mismatch"), ".zcode-plugin", "plugin.json"), {
      name: "other-name",
    });
    await write(join(plugin("badname"), ".zcode-plugin", "plugin.json"), {
      name: "Bad Name",
    });
    await mkdir(plugin("bare"), { recursive: true });
    await mkdir(plugin("loose"), { recursive: true });
    await write(join(market, "marketplace.json"), {
      name: "acme",
      plugins: [
        { name: "good", source: "./plugins/good" },
        { name: "risky", source: "./plugins/risky" },
        { name: "escape", source: "./plugins/escape" },
        { name: "mismatch", source: "./plugins/mismatch" },
        { name: "badname", source: "./plugins/badname" },
        { name: "bare", source: "./plugins/bare" },
        {
          name: "loose",
          source: "./plugins/loose",
          strict: false,
          version: "2.0.0",
          outputStyles: "./styles",
        },
        { name: "needy", source: "./plugins/good", dependencies: ["ghost"] },
        { name: "nosource" },
        { name: "pipsrc", source: { source: "pip", package: "x" } },
      ],
    });

    // source 校验用的独立市场目录（不落盘到存储）。
    const external = join(root, "markets", "ext");
    await write(join(external, "marketplace.json"), {
      name: "ext",
      plugins: [
        { name: "local", source: "./local" },
        { name: "remote", source: { source: "github", repo: "acme/remote" } },
        { name: "giturl", source: { source: "git", url: "https://example.invalid/r.git" } },
        { name: "zipnosha", source: { source: "url", type: "zip", url: "https://x/a.zip" } },
        { name: "ftp", source: { source: "url", type: "ftp", url: "ftp://x" } },
        { name: "npmsrc", source: { source: "npm", package: "x" } },
        { name: "nodir", source: "./missing" },
        { name: "deps", source: "./local", dependencies: ["local", "nope@elsewhere"] },
        { name: "  ", source: "./local" },
      ],
    });
    await write(join(external, "local", ".zcode-plugin", "plugin.json"), {
      name: "local",
      channels: {},
    });
    const empty = join(root, "markets", "empty");
    await write(join(empty, "marketplace.json"), { name: "empty", plugins: [] });

    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrubText = (text: string) =>
      text
        .replaceAll(rootText, "<root>")
        .replace(/zcode-plugin-src-[A-Za-z0-9-]+/g, "zcode-plugin-src-<tmp>");
    const call = async (params: object) => {
      try {
        return JSON.parse(
          scrubText(
            JSON.stringify(
              await h.client.request(
                "plugins/validate" as any,
                { workspace, ...params },
                z.any(),
              ),
            ),
          ),
        );
      } catch (error) {
        return {
          error: scrubText(error instanceof Error ? error.message : String(error)),
        };
      }
    };
    const observation: Record<string, unknown> = {};
    for (const name of [
      "good",
      "risky",
      "escape",
      "mismatch",
      "badname",
      "bare",
      "loose",
      "needy",
      "nosource",
      "pipsrc",
      "ghost",
    ]) {
      observation[name] = await call({ marketplace: "acme", pluginName: name });
    }
    observation.unknownMarket = await call({ marketplace: "nowhere", pluginName: "x" });
    observation.sourceExternal = await call({ source: external });
    observation.sourceEmpty = await call({ source: empty });
    observation.sourceMissing = await call({ source: join(root, "markets", "nope") });
    observation.sourceInvalid = await call({ source: "not a source" });
    observation.nothing = await call({});
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust validate plugins the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  // 自检：样例覆盖了预期的诊断。
  assert.equal((node.good as any).ok, true);
  assert.deepEqual((node.good as any).diagnostics, []);
  assert.equal((node.risky as any).ok, false);
  assert.equal((node.nothing as any).ok, true);
  const codes = (key: string) =>
    (node[key] as any).diagnostics.map((d: any) => d.code);
  assert.ok(codes("sourceExternal").includes("plugin_validation_deferred"));
  assert.ok(codes("needy").includes("plugin_dependency_missing"));
  assert.ok(codes("risky").includes("plugin_variable_missing"));
});
