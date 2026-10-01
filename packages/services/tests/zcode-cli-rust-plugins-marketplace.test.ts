import assert from "node:assert/strict";
import test from "node:test";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createServer, type Server } from "node:http";
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
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W4：plugins/marketplace/add|remove|update 与安装时按需拉取。
// 只用本地源（目录 / .json 文件 / 回环 URL / 本地 git 仓库声明），不刷新官方 CDN 市场。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function write(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}
const git = (cwd: string, ...args: string[]) =>
  execFileSync(
    "git",
    [
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@e",
      "-c",
      "commit.gpgsign=false",
      ...args,
    ],
    { cwd, encoding: "utf8" },
  ).trim();

async function tree(
  dir: string,
  scrub: (text: string) => string = (t) => t,
): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  const walk = async (current: string) => {
    let entries;
    try {
      entries = await readdir(current, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (entry.name === ".git") continue;
      const path = join(current, entry.name);
      if (entry.isDirectory()) await walk(path);
      else
        out[relative(dir, path).replaceAll("\\", "/")] = createHash("sha256")
          .update(scrub(await readFile(path, "utf8")))
          .digest("hex")
          .slice(0, 16);
    }
  };
  await walk(dir);
  return out;
}

const pluginManifest = (name: string) =>
  JSON.stringify({ name, version: "1.0.0", description: name });

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-market-${kind}-`));
  await mkdir(join(root, ".zcode", "cli"), { recursive: true });
  // 目录市场：对象写法的 plugins（会被规范化成数组），自带插件目录。
  const dirMarket = join(root, "markets", "dirmk");
  await write(
    join(dirMarket, "marketplace.json"),
    JSON.stringify({
      name: " dirmk ",
      metadata: { description: "Directory market" },
      plugins: { alpha: { source: "./plugins/alpha", version: "1.0.0" } },
      featured: ["alpha"],
    }),
  );
  await write(
    join(dirMarket, "plugins", "alpha", ".zcode-plugin", "plugin.json"),
    pluginManifest("alpha"),
  );
  const fileMarket = join(root, "markets", "filemk", "catalog.json");
  await write(
    fileMarket,
    JSON.stringify({
      name: "filemk",
      description: "File market",
      plugins: [{ name: "beta", source: "./beta" }],
    }),
  );
  await write(
    join(root, "markets", "filemk", "beta", ".zcode-plugin", "plugin.json"),
    pluginManifest("beta"),
  );
  const reserved = join(root, "markets", "reserved");
  await write(
    join(reserved, "marketplace.json"),
    JSON.stringify({ name: "zcode-plugins-official", plugins: [] }),
  );
  const invalid = join(root, "markets", "invalid");
  await write(
    join(invalid, "marketplace.json"),
    JSON.stringify({ name: "Bad Name", plugins: [] }),
  );
  const repo = join(root, "markets", "gitmk");
  await write(
    join(repo, ".claude-plugin", "marketplace.json"),
    JSON.stringify({
      name: "gitmk",
      plugins: [{ name: "gamma", source: "./gamma" }],
    }),
  );
  await write(
    join(repo, "gamma", ".zcode-plugin", "plugin.json"),
    pluginManifest("gamma"),
  );
  git(repo, "init", "-q", "-b", "main");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "c1");

  let urlStatus = 200;
  const server: Server = createServer((req, res) => {
    if (urlStatus !== 200) {
      res.writeHead(urlStatus);
      res.end();
      return;
    }
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        name: "urlmk",
        plugins: [
          {
            name: "delta",
            source: {
              source: "directory",
              path: join(root, "markets", "filemk", "beta"),
            },
          },
        ],
      }),
    );
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const port = (server.address() as { port: number }).port;

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
    const userConfigPath = join(root, ".zcode", "cli", "config.json");
    const before = JSON.parse(
      await readFile(userConfigPath, "utf8").catch(() => "{}"),
    );
    await write(
      userConfigPath,
      JSON.stringify({
        ...before,
        plugins: {
          extraKnownMarketplaces: {
            gitmk: { source: { source: "git", url: repo } },
          },
        },
      }),
    );
    const storage = join(root, ".zcode", "cli", "plugins");
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrubText = (text: string) =>
      text
        .replaceAll(rootText, "<root>")
        .replaceAll(root, "<root>")
        .replaceAll(String(port), "<port>")
        .replace(/20\d\d-\d\d-\d\dT[\d:.]+Z/g, "<time>")
        .replace(
          /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g,
          "<uuid>",
        )
        .replace(/zcode-plugin-src-[A-Za-z0-9-]+/g, "zcode-plugin-src-<tmp>");
    const scrub = (value: unknown) =>
      JSON.parse(scrubText(JSON.stringify(value)));
    const call = async (method: string, params: object) => {
      try {
        return scrub(
          await h.client.request(
            method as any,
            { workspace, ...params },
            z.any(),
          ),
        );
      } catch (error) {
        return {
          error: scrubText(
            error instanceof Error ? error.message : String(error),
          ),
        };
      }
    };
    const observation: Record<string, unknown> = {};
    observation.addDir = await call("plugins/marketplace/add", {
      source: dirMarket,
    });
    observation.addFile = await call("plugins/marketplace/add", {
      source: fileMarket,
    });
    observation.addUrl = await call("plugins/marketplace/add", {
      source: `http://127.0.0.1:${port}/market.json`,
    });
    observation.addDry = await call("plugins/marketplace/add", {
      source: dirMarket,
      dryRun: true,
    });
    observation.addReserved = await call("plugins/marketplace/add", {
      source: reserved,
    });
    observation.addInvalid = await call("plugins/marketplace/add", {
      source: invalid,
    });
    observation.addMissing = await call("plugins/marketplace/add", {
      source: join(root, "markets", "nope"),
    });
    observation.updateDeclaredGit = await call("plugins/marketplace/update", {
      marketplace: "gitmk",
    });
    observation.updateDir = await call("plugins/marketplace/update", {
      marketplace: "dirmk",
    });
    urlStatus = 500;
    observation.updateUrlFailing = await call("plugins/marketplace/update", {
      marketplace: "urlmk",
    });
    observation.updateUnknown = await call("plugins/marketplace/update", {
      marketplace: "ghost",
    });
    urlStatus = 200;
    // 按需拉取：删掉本地 urlmk 目录后安装其中插件，安装前会用已知记录的 source 重新拉取。
    await rm(join(storage, "marketplaces", "urlmk"), {
      recursive: true,
      force: true,
    });
    observation.installFromUrl = await call("plugins/install", {
      pluginName: "delta",
      marketplace: "urlmk",
    });
    observation.removeFile = await call("plugins/marketplace/remove", {
      marketplace: "filemk",
    });
    observation.known = scrubText(
      await readFile(join(storage, "known_marketplaces.json"), "utf8"),
    );
    observation.marketplaces = Object.fromEntries(
      Object.entries(
        await tree(join(storage, "marketplaces"), scrubText),
      ).filter(([path]) => !path.startsWith("zcode-plugins-official/")),
    );
    const overview = (await h.client.request(
      "plugins/overview",
      { workspace },
      z.any(),
    )) as any;
    observation.overviewMarkets = scrub(
      overview.marketplaces.filter(
        (m: any) => m.id !== "zcode-plugins-official",
      ),
    );
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await new Promise<void>((done) => server.close(() => done()));
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust manage plugin marketplaces the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  // 自检。
  assert.equal((node.addDir as any).marketplace.id, "dirmk");
  assert.equal((node.addDir as any).marketplace.pluginCount, 1);
  assert.match(
    String((node.addReserved as any).error),
    /reserved for the official marketplace/,
  );
  assert.equal((node.updateDeclaredGit as any).marketplaces[0].id, "gitmk");
  assert.equal((node.updateUrlFailing as any).diagnostics.length, 1);
  assert.equal(
    (node.installFromUrl as any).installedPlugins[0].id,
    "delta@urlmk",
  );
});
