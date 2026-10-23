// Phase 3 golden freeze：rustHostHooksParity.test.ts 的 TS `createHooksService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 录制内容与测试一一对应：
// - `loaded`：TS `saveHooks` 写出配置后 `loadHooks` 的完整结果（`discoveredAt` 归一化）；
// - `saved`：TS `saveHooks` 落盘的 user / project 两份 config 原文（L5 逐字节基线）；
// - `trusted`：写入 TS 形状的 trust store 后 `loadHooks` 的完整结果。
// 绝对临时路径统一归一化为 `$FIXTURE_ROOT`，测试运行时替换为本次临时根。
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHooksService } from "../packages/services/src/node.ts";

const fixtureHooks = [
  {
    id: "user-hook",
    event: "SessionStart",
    matcher: "startup",
    type: "process",
    command: "node",
    args: ["user.mjs"],
    enabled: true,
    custom: { flag: true },
    location: { source: "escode", scope: "user" },
  },
  {
    id: "project-hook",
    event: "PreToolUse",
    matcher: "Bash",
    type: "command",
    command: "./hook.sh",
    async: true,
    shell: "bash",
    statusMessage: "Preparing",
    timeout: 12,
    enabled: true,
    location: { source: "escode", scope: "project" },
  },
  {
    id: "skip-legacy",
    event: "Stop",
    type: "command",
    command: "echo skip",
    enabled: true,
    location: { source: "agents", scope: "user" },
  },
];

const originalHome = process.env.HOME;
const originalUserProfile = process.env.USERPROFILE;

const root = await mkdtemp(join(tmpdir(), "rusthost-hooks-golden-"));
const home = join(root, "home");
const workspace = join(root, "workspace");
const userConfigPath = join(home, ".escode", "cli", "config.json");
const projectConfigPath = join(workspace, ".escode", "config.json");
const trustStorePath = join(home, ".escode", "security", "workspace-hook-trust-v1.json");

/** 与测试一致：JSON 线上语义（去掉 undefined）+ `discoveredAt` 归一化 + 绝对根归一化。 */
function normalize(value: unknown, fixtureRoot: string): unknown {
  const wire = JSON.parse(JSON.stringify(value)) as Record<string, unknown>;
  const snapshot = wire.workspaceHookSnapshot as Record<string, unknown> | undefined;
  if (snapshot) snapshot.discoveredAt = "<time>";
  return JSON.parse(JSON.stringify(wire).split(fixtureRoot).join("$FIXTURE_ROOT"));
}

const normalizeText = (value: string, fixtureRoot: string) =>
  value.split(fixtureRoot).join("$FIXTURE_ROOT");

try {
  await mkdir(join(home, ".escode", "cli"), { recursive: true });
  await mkdir(join(workspace, ".escode"), { recursive: true });
  process.env.HOME = home;
  process.env.USERPROFILE = home;

  const ts = createHooksService();
  // TS 写出的存量文件（未知键 features 必须保留）。
  await writeFile(userConfigPath, JSON.stringify({ features: { mcp: true } }));
  await ts.saveHooks({ workspacePath: workspace, hooks: fixtureHooks } as never);

  const saved = {
    userConfig: normalizeText(await readFile(userConfigPath, "utf8"), root),
    projectConfig: normalizeText(await readFile(projectConfigPath, "utf8"), root),
  };
  const loaded = normalize(
    await ts.loadHooks({ workspacePath: workspace, workspaceIdentity: workspace }),
    root,
  ) as {
    workspaceHookSnapshot: {
      bundleDigest: string;
      hooks: Array<{ hookDeclarationDigest: string }>;
    };
  };

  // trust store 存量读入：TS 形状的持久信任命中（T4）。
  const snapshot = loaded.workspaceHookSnapshot;
  await mkdir(join(home, ".escode", "security"), { recursive: true });
  await writeFile(
    trustStorePath,
    JSON.stringify({
      schemaVersion: 1,
      records: [
        {
          workspaceIdentity: workspace,
          hookDeclarationDigest: snapshot.hooks[0].hookDeclarationDigest,
          digestAlgorithm: "sha256",
          decision: "trusted",
          grantedAt: new Date().toISOString(),
          eventAtGrant: "PreToolUse",
          displayCommandAtGrant: "./hook.sh",
          sourcePathAtGrant: ".escode/config.json",
        },
      ],
    }),
  );
  const trusted = normalize(
    await ts.loadHooks({ workspacePath: workspace, workspaceIdentity: workspace }),
    root,
  );

  const golden = {
    _readme:
      "rustHostHooksParity.test.ts 的 TS createHooksService 观察冻结（Phase 3 golden freeze）。discoveredAt 归一化为 <time>；绝对临时根归一化为 $FIXTURE_ROOT，测试运行时替换。",
    expected: { loaded, saved, trusted },
  };

  const goldenDir = "packages/desktop/test/fixtures/golden";
  const jsonPath = join(goldenDir, "rustHostHooksParity.oracle.json");
  const json = `${JSON.stringify(golden, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    const { deepEqual } = await import("node:assert/strict");
    deepEqual(
      JSON.parse(await readFile(jsonPath, "utf8")),
      JSON.parse(json),
      "rustHostHooksParity golden 与当前 TS 观察漂移",
    );
    console.log("rustHostHooksParity golden: OK");
  } else {
    await mkdir(goldenDir, { recursive: true });
    await writeFile(jsonPath, json);
    console.log("wrote", jsonPath, "root=", root);
  }
} finally {
  process.env.HOME = originalHome;
  if (originalUserProfile === undefined) delete process.env.USERPROFILE;
  else process.env.USERPROFILE = originalUserProfile;
  await rm(root, { recursive: true, force: true });
}
