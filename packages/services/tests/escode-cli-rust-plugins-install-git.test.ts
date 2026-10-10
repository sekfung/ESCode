import assert from "node:assert/strict";
import test from "node:test";
import { execFileSync } from "node:child_process";
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

// docs/specs/rust-plugin-marketplace-write.md W3：仓库类源。非 GitHub URL 走 Archive → 系统 Git 回退：
// 普通 clone、ref（分支）、sha pin（检出旧提交）、git-subdir、子目录缺失、仓库不存在、Git 不可用。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

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
    {
      cwd,
      encoding: "utf8",
    },
  ).trim();

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
      if (entry.name === ".git") continue;
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

async function makeRepo(dir: string) {
  await mkdir(dir, { recursive: true });
  git(dir, "init", "-q", "-b", "main");
  await write(
    join(dir, ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "tool", version: "1.0.0" }),
  );
  await write(
    join(dir, "packages", "sub", ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "sub", version: "0.3.0" }),
  );
  git(dir, "add", "-A");
  git(dir, "commit", "-q", "-m", "c1");
  const first = git(dir, "rev-parse", "HEAD");
  await write(
    join(dir, ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "tool", version: "1.1.0" }),
  );
  git(dir, "commit", "-q", "-am", "c2");
  git(dir, "checkout", "-q", "-b", "v2");
  await write(
    join(dir, ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "tool", version: "2.0.0" }),
  );
  git(dir, "commit", "-q", "-am", "c3");
  git(dir, "checkout", "-q", "main");
  return first;
}

async function observe(kind: Runtime, gitBinary?: string) {
  const root = await mkdtemp(join(tmpdir(), `escode-plugins-git-${kind}-`));
  await mkdir(join(root, ".escode", "cli"), { recursive: true });
  const repo = join(root, "repos", "tool");
  const first = await makeRepo(repo);
  const env = {
    ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ESCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
    ...(gitBinary ? { ESCODE_GIT_BINARY: gitBinary } : {}),
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
    await write(
      join(storage, "marketplaces", "acme", "marketplace.json"),
      JSON.stringify({
        name: "acme",
        plugins: [
          { name: "gitplain", source: { source: "git", url: repo } },
          { name: "gitref", source: { source: "git", url: repo, ref: "v2" } },
          { name: "gitsha", source: { source: "git", url: repo, sha: first } },
          {
            name: "subdir",
            source: { source: "git-subdir", url: repo, path: "packages/sub" },
          },
          {
            name: "missingsub",
            source: { source: "git-subdir", url: repo, path: "nope" },
          },
          {
            name: "badrepo",
            source: { source: "git", url: join(root, "repos", "missing") },
          },
          { name: "nourl", source: { source: "git" } },
        ],
      }),
    );
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const rootText = JSON.stringify(root).slice(1, -1);
    const scrubText = (text: string) =>
      text
        .replaceAll(rootText, "<root>")
        .replaceAll(first, "<sha>")
        // clone 临时目录名的随机后缀（Node mkdtemp vs Rust uuid）不确定。
        .replace(/escode-plugin-src-[A-Za-z0-9-]+/g, "escode-plugin-src-<tmp>")
        .replace(/20\d\d-\d\d-\d\dT[\d:.]+Z/g, "<time>")
        .replace(
          /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g,
          "<uuid>",
        );
    const names = gitBinary
      ? ["gitplain"]
      : [
          "gitplain",
          "gitref",
          "gitsha",
          "subdir",
          "missingsub",
          "badrepo",
          "nourl",
        ];
    const observation: Record<string, unknown> = {};
    for (const name of names) {
      const result = (await h.client.request(
        "plugins/install",
        { workspace, pluginName: name, marketplace: "acme" },
        z.any(),
      )) as any;
      observation[name] = JSON.parse(
        scrubText(
          JSON.stringify({
            closure: result.dependencyClosure,
            installed: result.installedPlugins,
            // git 子进程报错的原文来自各自的进程封装（execFile vs Rust），只比对 code；其余逐字。
            diagnostics: result.diagnostics.map((d: any) => ({
              code: d.code,
              message: d.message.startsWith("Command failed")
                ? "<git failure>"
                : d.message,
            })),
          }),
        ),
      );
    }
    if (!gitBinary) {
      observation.installedFile = scrubText(
        await readFile(join(storage, "installed_plugins.json"), "utf8"),
      );
      observation.cache = await tree(join(storage, "cache", "acme"));
    }
    observation.schemaErrors = h.schemaErrors;
    await h.close();
    return observation;
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("Node and Rust install plugins from git sources the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  for (const key of Object.keys(node)) {
    assert.deepEqual(rust[key], node[key], key);
  }
  assert.deepEqual(node.schemaErrors, []);
  const version = (name: string) => (node[name] as any).installed[0]?.version;
  assert.equal(version("gitplain"), "1.1.0");
  assert.equal(version("gitref"), "2.0.0");
  assert.equal(version("gitsha"), "1.0.0");
  assert.equal(version("subdir"), "0.3.0");
  for (const name of ["missingsub", "badrepo", "nourl"]) {
    assert.equal((node[name] as any).diagnostics.length, 1, name);
  }
});

test("Node and Rust report an unavailable system Git the same way", async () => {
  const missing = join(tmpdir(), "escode-no-such-git", "git.exe");
  const node = await observe("node", missing);
  const rust = await observe("rust", missing);
  assert.deepEqual(rust, node);
  assert.equal(
    (node.gitplain as any).diagnostics[0].code,
    "plugin_git_unavailable",
  );
});
