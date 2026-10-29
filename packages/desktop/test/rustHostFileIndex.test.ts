import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// Phase 3 golden freeze：TS `createFileService` 的索引/搜索/忽略规则观察已冻结为仓内资产
// （packages/desktop/test/fixtures/golden/rustHostFileIndex.oracle.json）。测试不再实例化 TS 服务，
// 删除 TS 实现后仍能独立重验 Rust Host 的 `file` channel（真实 `@escode/client`）。
const goldenFile = new URL("./fixtures/golden/rustHostFileIndex.oracle.json", import.meta.url);

// 搜索打分会把绝对 rootPath 参与模糊匹配（path 维度），因此录制与回放必须使用同一个根路径
// 字符串；mkdtemp 的随机段会改变命中集合。固定 POSIX 路径（win32 测试已跳过）保证跨机器一致。
const FIXTURE_ROOT = "/tmp/escode-rust-host-file-index-fixture";
const WORK_ROOT = join(FIXTURE_ROOT, "work");

type FileService = Awaited<ReturnType<typeof connectRpc>>["fileService"];

// docs/specs/rust-u2-file-index.md §5 TS oracle 差分：同一组夹具目录交给 Rust Host 的 `file` channel，
// 索引内容、搜索结果、规则读写与冻结的 TS 观察逐项全等。

/** 夹具树：覆盖隐藏目录、默认黑名单、二进制扩展名、.env、中文/空格/TAB/反斜杠/emoji 文件名与软链接。 */
const TREE_FILES = [
  "README.md",
  "src/main.ts",
  "src/lib/util.ts",
  "src/lib/util.test.ts",
  "src/app/页面.tsx",
  "src/app/Page With Space.tsx",
  "src/emoji/😀-file.md",
  "docs/guide/intro.md",
  "docs/Guide/Intro.MD",
  ".github/workflows/ci.yml",
  ".env",
  ".env.local",
  "node_modules/pkg/index.js",
  "build/out.js",
  "build/keep.txt",
  "dist/bundle.js",
  "lib/native.so",
  "lib/helper.o",
  "coverage/lcov.info",
  "coverage.out",
  "logs/app.log",
  "logs/archive/old.log",
  "tab\tname.txt",
  "back\\slash.txt",
  "a/b/c/d/deep.txt",
  "a/b.txt",
  "temp.tmp",
  "keep/temp.tmp",
  "Makefile",
  "cmake-build-debug/x.txt",
  "__pycache__/m.pyc",
];

async function buildTree(root: string): Promise<void> {
  for (const file of TREE_FILES) {
    const path = join(root, file);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, "x");
  }
  await mkdir(join(root, "empty-dir"), { recursive: true });
  await symlink(join(root, "src"), join(root, "link-to-src"));
  await symlink(join(root, "README.md"), join(root, "link-to-readme"));
  await symlink(join(root, "missing-target"), join(root, "dangling-link"));
}

const RULE_SETS: Array<[string, string]> = [
  ["no rules file", ""],
  ["basic names", "*.log\nbuild/\n"],
  ["negation", "*.log\n!logs/archive/old.log\n"],
  ["anchored", "/temp.tmp\n"],
  ["unanchored", "temp.tmp\n"],
  ["double star", "**/deep.txt\nsrc/**/util.ts\n"],
  ["leading double star dir", "**/lib/\n"],
  ["dir only vs file", "build\n"],
  ["parent excluded cannot be re-included", "build/\n!build/keep.txt\n"],
  ["parent file-level negation works", "build/*\n!build/keep.txt\n"],
  ["char class", "util.[tj]s\n[Dd]ocs/\n"],
  ["question mark", "?.txt\na/?.txt\n"],
  ["escaped hash and bang", "\\#notcomment\n\\!literal\n"],
  ["comment and blank lines", "# comment\n\n   \n*.tmp\n"],
  ["trailing spaces", "temp.tmp   \nlogs/   \n"],
  ["CRLF line endings", "*.log\r\nbuild/\r\n"],
  ["case sensitivity", "readme.md\nIntro.md\n"],
  ["unicode names", "页面.tsx\n*.md\n"],
  ["emoji", "😀-*\n"],
  ["backslash name", "back\\\\slash.txt\n"],
  ["everything then re-include", "*\n!src/\n!src/**\n"],
  ["ignore all files keep dirs", "*\n!*/\n"],
  ["nested glob", "src/*/util*.ts\n"],
  ["root-relative with slash", "docs/guide\n"],
  ["trailing double star", "docs/**\n"],
  ["hidden dir rules", ".github/\n"],
  ["empty-dir rule", "empty-dir/\n"],
  ["symlink name rule", "link-to-src\n"],
  ["symlink dir slash rule", "link-to-src/\n"],
  ["defaults only template", "node_modules/\n.git/\n"],
  ["re-include node_modules", "!node_modules/\n"],
  ["invalid bracket", "[abc\nREADME.md\n"],
];

const QUERIES = [
  "",
  "  ",
  "main",
  "MAIN",
  "util",
  "util.ts",
  "ts",
  "test",
  "src",
  "src/lib",
  "lib/util",
  "页面",
  "页",
  "😀",
  "file.md",
  "space",
  "page with",
  "intro",
  "guide",
  "ci",
  "yml",
  "deep",
  "d",
  "zz",
  "m.t",
  "readme",
  "mk",
  "u.t",
  "s/l/u",
  "a/b",
];

const LIMITS = [undefined, 0, 1, 3, 1000, 5000, -2, 2.7];

async function drive(service: FileService, root: string): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};
  for (const [label, rules] of RULE_SETS) {
    const dir = join(root, `rules-${label.replace(/\W+/g, "-")}`);
    await mkdir(dir, { recursive: true });
    await buildTree(dir);
    if (label !== "no rules file") await writeFile(join(dir, ".escodeignore"), rules);
    // 与规则文件并存的 .gitignore：无 .escodeignore 时模板会拷贝它。
    await writeFile(join(dir, ".gitignore"), "*.tmp\nlogs/\n");
    const all = await service.searchWorkspaceFiles({
      rootPath: dir,
      query: "",
      limit: 1000,
    });
    out[`rules:${label}`] = all.map((entry) => entry.relativePath);
    const length = await service.listWorkspaceFilesLength({ rootPath: dir });
    let joined = "";
    for (let offset = 0; offset < length; offset += 97) {
      joined += await service.listWorkspaceFilesRange({
        rootPath: dir,
        offset,
        length: 97,
      });
    }
    out[`packed:${label}`] = { length, joined };
  }
  const dir = join(root, "search");
  await mkdir(dir, { recursive: true });
  await buildTree(dir);
  for (const query of QUERIES) {
    out[`search:${query}`] = await service.searchWorkspaceFiles({
      rootPath: dir,
      query,
      limit: 25,
    });
  }
  for (const limit of LIMITS) {
    out[`limit:${String(limit)}`] = await service.searchWorkspaceFiles({
      rootPath: dir,
      query: "t",
      ...(limit === undefined ? {} : { limit }),
    });
  }
  out.invalidLimit = await service
    // NaN/Infinity 经 JSON-RPC 会变成 null（走缺省值），所以用非数字类型触发同一个参数错误。
    .searchWorkspaceFiles({
      rootPath: dir,
      query: "t",
      limit: "5" as unknown as number,
    })
    .catch((error: Error) => error.message);
  out.invalidQuery = await service
    .searchWorkspaceFiles({ rootPath: dir, query: 5 as unknown as string })
    .catch((error: Error) => error.message);
  out.rangeEdges = [
    await service.listWorkspaceFilesRange({
      rootPath: dir,
      offset: -5,
      length: 10,
    }),
    await service.listWorkspaceFilesRange({
      rootPath: dir,
      offset: 1e9,
      length: 10,
    }),
    await service.listWorkspaceFilesRange({
      rootPath: dir,
      offset: 3.9,
      length: 4.9,
    }),
    await service.listWorkspaceFilesRange({
      rootPath: dir,
      offset: 0,
      length: -1,
    }),
  ];
  return out;
}

async function driveIgnoreSettings(
  service: FileService,
  root: string,
): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};
  const dir = join(root, "ignore-settings");
  await mkdir(dir, { recursive: true });
  const preview = (label: string) =>
    service.readWorkspaceFileSearchIgnore({ rootPath: dir }).then((value) => (out[label] = value));
  await preview("noFiles");
  await writeFile(join(dir, ".gitignore"), "dist/\n*.log");
  await preview("withGitignore");
  const template = (await service.readWorkspaceFileSearchIgnore({ rootPath: dir })).content;
  out.syncFromTemplate = await service.applyWorkspaceFileSearchIgnoreTransform({
    rootPath: dir,
    transform: "sync-gitignore",
  });
  out.resetFromTemplate = await service.applyWorkspaceFileSearchIgnoreTransform({
    rootPath: dir,
    transform: "reset-defaults",
  });
  await service.writeWorkspaceFileSearchIgnore({
    rootPath: dir,
    content: `${template}my-custom/\n`,
  });
  await preview("afterWrite");
  await writeFile(join(dir, ".gitignore"), "build/\nnode_modules\n");
  out.syncKeepsCustom = await service.applyWorkspaceFileSearchIgnoreTransform({
    rootPath: dir,
    transform: "sync-gitignore",
  });
  out.resetKeepsCustom = await service.applyWorkspaceFileSearchIgnoreTransform({
    rootPath: dir,
    transform: "reset-defaults",
  });
  // 标记缺失：整体按模板重建。
  await service.writeWorkspaceFileSearchIgnore({
    rootPath: dir,
    content: "only-custom/\n",
  });
  out.syncNoMarkers = await service.applyWorkspaceFileSearchIgnoreTransform({
    rootPath: dir,
    transform: "sync-gitignore",
  });
  out.resetNoMarkers = await service.applyWorkspaceFileSearchIgnoreTransform({
    rootPath: dir,
    transform: "reset-defaults",
  });
  // 无尾换行 / 空 gitignore。
  await writeFile(join(dir, ".gitignore"), "   \n");
  await service.writeWorkspaceFileSearchIgnore({
    rootPath: dir,
    content: `${template.trimEnd()}`,
  });
  out.syncNoTrailingNewline = await service.applyWorkspaceFileSearchIgnoreTransform({
    rootPath: dir,
    transform: "sync-gitignore",
  });
  out.written = await readFile(join(dir, ".escodeignore"), "utf8");
  return out;
}

describe("rust host file index domain", () => {
  it.skipIf(!existsSync(hostBinary) || process.platform === "win32")(
    "matches the TS file service for search, packing and ignore rules",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-index-")));
      const home = join(base, "home");
      const host = startHost(home, { LANG: "zh_CN.UTF-8", LC_ALL: "zh_CN.UTF-8" });
      try {
        const root = WORK_ROOT;
        await rm(FIXTURE_ROOT, { recursive: true, force: true });
        await mkdir(root, { recursive: true });
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));

        // 冻结观察中的绝对临时路径统一写成 $FIXTURE_ROOT；替换成本次运行的工作根后逐字段比较。
        const golden = JSON.parse(
          (await readFile(goldenFile, "utf8")).split("$FIXTURE_ROOT").join(root),
        ) as { expected: Record<string, unknown>; settings: unknown };
        const rust = services.fileService;

        const actual = await drive(rust, root);
        const settingsActual = await driveIgnoreSettings(rust, root);
        for (const key of Object.keys(golden.expected)) {
          expect({ key, value: actual[key] }).toEqual({
            key,
            value: golden.expected[key],
          });
        }
        expect(Object.keys(actual)).toEqual(Object.keys(golden.expected));
        expect(settingsActual).toEqual(golden.settings);
      } finally {
        await host.stop();
        await rm(FIXTURE_ROOT, { recursive: true, force: true });
        await rm(base, { recursive: true, force: true });
      }
    },
    300_000,
  );
});
