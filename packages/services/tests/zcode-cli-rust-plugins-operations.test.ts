import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { createServer, type Server } from "node:http";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import yazl from "yazl";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W5：plugins/update（按 id / 市场 / 全部重装）与
// plugins/cancelOperation（未知 id；进行中的安装被取消）。慢下载期间 plugins/list 仍能应答（不阻塞其它请求）。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function write(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}
function makeZip(files: Record<string, string>): Promise<Buffer> {
  return new Promise((done, reject) => {
    const zip = new yazl.ZipFile();
    for (const [path, content] of Object.entries(files)) {
      zip.addBuffer(Buffer.from(content), path, { mtime: new Date(0) });
    }
    zip.end();
    const chunks: Buffer[] = [];
    zip.outputStream.on("data", (chunk: Buffer) => chunks.push(chunk));
    zip.outputStream.on("end", () => done(Buffer.concat(chunks)));
    zip.outputStream.on("error", reject);
  });
}

async function observe(kind: Runtime) {
  const slowZip = await makeZip({
    ".zcode-plugin/plugin.json": JSON.stringify({
      name: "slow",
      version: "1.0.0",
    }),
  });
  let release: () => void = () => {};
  const released = new Promise<void>((done) => (release = done));
  const server: Server = createServer(async (_req, res) => {
    await released;
    res.writeHead(200, { "content-type": "application/zip" });
    res.end(slowZip);
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const port = (server.address() as { port: number }).port;

  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-ops-${kind}-`));
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
    for (const name of ["tool", "other"]) {
      await write(
        join(market, "plugins", name, ".zcode-plugin", "plugin.json"),
        JSON.stringify({ name, version: "1.0.0" }),
      );
    }
    await write(
      join(market, "marketplace.json"),
      JSON.stringify({
        name: "acme",
        plugins: [
          { name: "tool", source: "./plugins/tool" },
          { name: "other", source: "./plugins/other" },
          {
            name: "slow",
            source: {
              source: "url",
              type: "zip",
              url: `http://127.0.0.1:${port}/slow.zip`,
              sha256: createHash("sha256").update(slowZip).digest("hex"),
            },
          },
        ],
      }),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrub = (value: unknown) =>
      JSON.parse(
        JSON.stringify(value)
          .replaceAll(rootText, "<root>")
          .replace(/20\d\d-\d\d-\d\dT[\d:.]+Z/g, "<time>"),
      );
    const request = (method: string, params: object) =>
      h.client.request(
        method as any,
        { workspace, ...params },
        z.any(),
      ) as Promise<any>;
    // cancelOperation 的协议参数只有 operationId（不带 workspace）。
    const cancel = (operationId: string) =>
      h.client.request(
        "plugins/cancelOperation" as any,
        { operationId },
        z.any(),
      ) as Promise<any>;
    const observation: Record<string, unknown> = {};
    for (const name of ["tool", "other"]) {
      await request("plugins/install", {
        pluginName: name,
        marketplace: "acme",
      });
    }
    // 源目录升版后重装：按 id、按市场、全部。
    await write(
      join(market, "plugins", "tool", ".zcode-plugin", "plugin.json"),
      JSON.stringify({ name: "tool", version: "1.1.0" }),
    );
    observation.updateById = scrub(
      await request("plugins/update", { pluginId: "tool@acme" }),
    );
    observation.updateByMarket = scrub(
      await request("plugins/update", { marketplace: "acme" }),
    );
    observation.updateAll = scrub(await request("plugins/update", {}));
    observation.updateNone = scrub(
      await request("plugins/update", { pluginId: "ghost@acme" }),
    );
    observation.installedFile = (
      await readFile(join(storage, "installed_plugins.json"), "utf8")
    )
      .replaceAll(rootText, "<root>")
      .replace(/20\d\d-\d\d-\d\dT[\d:.]+Z/g, "<time>")
      .replace(
        /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g,
        "<uuid>",
      );
    observation.cancelUnknown = await cancel("nope");
    // 进行中的安装（只在 Rust 上断言）：Node 的 app-server 在插件安装进行中不应答其它请求（plugins/list、
    // plugins/cancelOperation 都会超时），无法做同步差分；Rust 以后台作业执行，期间其它请求照常应答，
    // 取消在下一个安全点生效（下载完成后、物化前），安装不落盘。
    if (kind === "rust") {
      const pending = h.client.request(
        "plugins/install" as any,
        {
          workspace,
          pluginName: "slow",
          marketplace: "acme",
          operationId: "op-slow",
        },
        z.any(),
        { timeoutMs: 60_000 },
      ) as Promise<any>;
      await new Promise((done) => setTimeout(done, 500));
      const listed = await request("plugins/list", {});
      const inFlight = {
        listDuringInstall: Array.isArray(listed.plugins),
        cancelSlow: await cancel("op-slow"),
        cancelAgain: await cancel("op-slow"),
      };
      release();
      const cancelled = await pending;
      observation.inFlight = {
        ...inFlight,
        installed: cancelled.installedPlugins,
        messages: cancelled.diagnostics.map((d: any) => d.message),
        slowNotInstalled: !(
          await readFile(join(storage, "installed_plugins.json"), "utf8")
        ).includes("slow@acme"),
      };
    }
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    release();
    await f.close();
    await new Promise<void>((done) => server.close(() => done()));
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust update plugins and answer cancelOperation the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  const { inFlight, ...shared } = rust;
  assert.deepEqual(shared, node);
  assert.deepEqual(node.schemaErrors, []);
  assert.equal((node.updateById as any).installedPlugins[0].version, "1.1.0");
  assert.equal((node.updateAll as any).installedPlugins.length, 2);
  assert.deepEqual(node.cancelUnknown, {
    operationId: "nope",
    cancelled: false,
  });
  // Rust 独有：慢安装不阻塞其它请求，可按 operationId 取消。
  assert.deepEqual(inFlight, {
    listDuringInstall: true,
    cancelSlow: { operationId: "op-slow", cancelled: true },
    cancelAgain: { operationId: "op-slow", cancelled: false },
    installed: [],
    messages: ["Plugin operation cancelled"],
    slowNotInstalled: true,
  });
});
