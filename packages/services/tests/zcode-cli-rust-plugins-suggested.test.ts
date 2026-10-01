import assert from "node:assert/strict";
import test from "node:test";
import { createServer, type Server } from "node:http";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W5b：plugins/resolveSuggestedReference。非官方 / 非法 id 直接
// unavailable；本地已有（内置官方插件）返回 ready/disabled；否则先发 plugins/operationProgress refreshing，
// 刷新官方目录（本地回环服务充当 CDN）后返回 missing（带 listing）、未列出或刷新失败。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const OFFICIAL = "zcode-plugins-official";

async function write(path: string, content: unknown) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, typeof content === "string" ? content : JSON.stringify(content));
}

async function observe(kind: Runtime) {
  let status = 200;
  const server: Server = createServer((_req, res) => {
    if (status !== 200) {
      res.writeHead(status);
      res.end();
      return;
    }
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        name: OFFICIAL,
        plugins: [
          {
            name: "cdnplug",
            description: "From CDN",
            icon: " https://icons/cdn.png ",
            displayName: "CDN Plug",
            source: { source: "url", type: "zip", url: "https://x/a.zip", sha256: "0".repeat(64) },
          },
        ],
      }),
    );
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const port = (server.address() as { port: number }).port;

  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-suggested-${kind}-`));
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
    await write(join(storage, "known_marketplaces.json"), {
      version: 1,
      marketplaces: [
        {
          id: OFFICIAL,
          source: { source: "url", url: `http://127.0.0.1:${port}/marketplace.json` },
          name: OFFICIAL,
          addedAt: "2026-01-01T00:00:00.000Z",
          pluginCount: 0,
        },
      ],
    });
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    let op = 0;
    const resolveRef = async (stableId: string) => {
      const operationId = `op-${++op}`;
      const before = h.messages.length;
      const result = await h.client.request(
        "plugins/resolveSuggestedReference" as any,
        {
          workspace,
          stableId,
          operationId,
          clientMode: "desktop-continuous",
          deliveryKind: "desktop-continuous",
        },
        z.any(),
        { timeoutMs: 30_000 },
      );
      const progress = h.messages
        .slice(before)
        .filter((m: any) => m.method === "plugins/operationProgress")
        .map((m: any) => m.params);
      return { result, progress };
    };
    const catalog = (await h.client.request(
      "plugins/referenceCatalog" as any,
      { workspace },
      z.any(),
    )) as any;
    const builtin = catalog.plugins.find((p: any) => p.marketplace === OFFICIAL);
    const observation: Record<string, unknown> = {};
    observation.builtinId = builtin?.pluginId ?? null;
    if (builtin) observation.builtin = await resolveRef(builtin.pluginId);
    observation.untrusted = await resolveRef("tool@acme");
    observation.malformed = await resolveRef("bad id@zcode-plugins-official");
    observation.noName = await resolveRef("@zcode-plugins-official");
    observation.missing = await resolveRef(`cdnplug@${OFFICIAL}`);
    observation.notListed = await resolveRef(`ghost@${OFFICIAL}`);
    status = 500;
    observation.refreshFailed = await resolveRef(`ghost2@${OFFICIAL}`);
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return JSON.parse(JSON.stringify(observation).replaceAll(String(port), "<port>"));
  } finally {
    await f.close();
    await new Promise<void>((done) => server.close(() => done()));
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust resolve suggested plugin references the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  assert.equal(node.untrusted.result.status, "unavailable");
  assert.deepEqual(node.untrusted.progress, []);
  assert.equal(node.missing.result.status, "missing");
  assert.equal(node.missing.progress.length, 1);
  assert.equal(node.missing.progress[0].state, "refreshing");
  assert.equal(node.notListed.result.diagnostics[0].code, "plugin_suggested_reference_not_listed");
  assert.equal(node.refreshFailed.result.diagnostics[0].code, "marketplace_refresh_failed");
});
