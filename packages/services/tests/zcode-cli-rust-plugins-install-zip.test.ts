import assert from "node:assert/strict";
import test from "node:test";
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
import yazl from "yazl";
import { z } from "zod";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-plugin-marketplace-write.md W2：zip 源安装。本地回环 HTTP 服务提供 zip（回环允许 http），
// 覆盖单顶层目录 strip、显式 path、同源重定向转发自定义头、sha256 不符、manifest 名不符、多顶层无 manifest、
// 非 HTTPS、禁用头；比对协议返回、安装记录、缓存文件树与服务端收到的请求。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

function makeZip(files: Record<string, string>): Promise<Buffer> {
  return new Promise((resolvePromise, reject) => {
    const zip = new yazl.ZipFile();
    for (const [path, content] of Object.entries(files)) {
      zip.addBuffer(Buffer.from(content), path, { mtime: new Date(0) });
    }
    zip.end();
    const chunks: Buffer[] = [];
    zip.outputStream.on("data", (chunk: Buffer) => chunks.push(chunk));
    zip.outputStream.on("end", () => resolvePromise(Buffer.concat(chunks)));
    zip.outputStream.on("error", reject);
  });
}
const sha = (bytes: Buffer) => createHash("sha256").update(bytes).digest("hex");
const manifest = (name: string, version = "1.0.0") =>
  JSON.stringify({ name, version });

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
        out[relative(dir, path).replaceAll("\\", "/")] = sha(
          await readFile(path),
        ).slice(0, 16);
    }
  };
  await walk(dir);
  return out;
}

async function observe(kind: Runtime) {
  const archives: Record<string, Buffer> = {
    "/zipped.zip": await makeZip({
      "zipped-1.0/.zcode-plugin/plugin.json": manifest("zipped"),
      "zipped-1.0/skills/s/SKILL.md": "---\nname: s\n---\nb\n",
    }),
    "/redirected.zip": await makeZip({
      ".zcode-plugin/plugin.json": manifest("redirected", "2.0.0"),
    }),
    "/withpath.zip": await makeZip({
      "pkg/inner/.zcode-plugin/plugin.json": manifest("withpath"),
      "pkg/other.txt": "x",
    }),
    "/mismatch.zip": await makeZip({
      ".zcode-plugin/plugin.json": manifest("other"),
    }),
    "/multi.zip": await makeZip({ "a/readme.md": "a", "b/readme.md": "b" }),
  };
  const requests: { path: string; market: string | undefined }[] = [];
  const server: Server = createServer((req, res) => {
    requests.push({
      path: req.url ?? "",
      market: req.headers["x-market"] as string | undefined,
    });
    if (req.url === "/redirect") {
      res.writeHead(302, { location: "/redirected.zip" });
      res.end();
      return;
    }
    const body = archives[req.url ?? ""];
    if (!body) {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, { "content-type": "application/zip" });
    res.end(body);
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const port = (server.address() as { port: number }).port;
  const base = `http://127.0.0.1:${port}`;
  const zipSource = (path: string, extra: Record<string, unknown> = {}) => ({
    source: "url",
    type: "zip",
    url: `${base}${path}`,
    sha256: sha(archives[path] ?? Buffer.alloc(0)),
    ...extra,
  });

  const root = await mkdtemp(join(tmpdir(), `zcode-plugins-zip-${kind}-`));
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
    await write(
      join(storage, "marketplaces", "acme", "marketplace.json"),
      JSON.stringify({
        name: "acme",
        plugins: [
          { name: "zipped", source: zipSource("/zipped.zip") },
          {
            name: "redirected",
            source: {
              ...zipSource("/redirected.zip"),
              url: `${base}/redirect`,
              headers: { "X-Market": "acme" },
            },
          },
          {
            name: "withpath",
            source: zipSource("/withpath.zip", { path: "pkg/inner" }),
          },
          {
            name: "badsha",
            source: { ...zipSource("/zipped.zip"), sha256: "0".repeat(64) },
          },
          { name: "mismatch", source: zipSource("/mismatch.zip") },
          { name: "multi", source: zipSource("/multi.zip") },
          {
            name: "nohttps",
            source: {
              ...zipSource("/zipped.zip"),
              url: "http://example.invalid/p.zip",
            },
          },
          {
            name: "denied",
            source: zipSource("/zipped.zip", {
              headers: { Authorization: "secret" },
            }),
          },
        ],
      }),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrubText = (text: string) =>
      text
        .replaceAll(rootText, "<root>")
        .replaceAll(String(port), "<port>")
        .replace(/20\d\d-\d\d-\d\dT[\d:.]+Z/g, "<time>")
        .replace(
          /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g,
          "<uuid>",
        );
    const scrub = (value: unknown) =>
      JSON.parse(scrubText(JSON.stringify(value)));
    const observation: Record<string, unknown> = {};
    for (const name of [
      "zipped",
      "redirected",
      "withpath",
      "badsha",
      "mismatch",
      "multi",
      "nohttps",
      "denied",
    ]) {
      const result = (await h.client.request(
        "plugins/install",
        { workspace, pluginName: name, marketplace: "acme" },
        z.any(),
      )) as any;
      observation[name] = scrub({
        closure: result.dependencyClosure,
        installed: result.installedPlugins,
        // 下载 / 解压错误文案里含各自 HTTP 栈的措辞时只比对 code；其余逐字。
        diagnostics: result.diagnostics.map((d: any) => ({
          code: d.code,
          message: /Failed to download/.test(d.message)
            ? "<download error>"
            : d.message,
        })),
      });
    }
    observation.installedFile = scrubText(
      await readFile(join(storage, "installed_plugins.json"), "utf8"),
    );
    observation.cache = await tree(join(storage, "cache", "acme"));
    observation.requests = requests.map((r) => ({ ...r }));
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await new Promise<void>((done) => server.close(() => done()));
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust install plugins from zip sources the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  // 自检。
  for (const name of ["zipped", "redirected", "withpath"]) {
    assert.equal((node[name] as any).installed.length, 1, name);
  }
  for (const name of ["badsha", "mismatch", "multi", "nohttps", "denied"]) {
    assert.equal((node[name] as any).diagnostics.length, 1, name);
  }
  assert.ok(
    (node.requests as any[]).some(
      (r) => r.path === "/redirected.zip" && r.market === "acme",
    ),
  );
});
