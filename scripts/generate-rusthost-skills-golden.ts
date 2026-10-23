// Phase 3 golden freeze：rustHostSkills.test.ts 的 TS `createSkillsService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：夹具根写成 `$FIXTURE_ROOT`（测试运行时归一到同一占位符）；skill id 的路径哈希写成 `<hash>`；
// audit log 的时间戳不参与比较（与测试一致地整体排除）。
import { deepEqual } from "node:assert/strict";
import {
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const md = (name: string | null, description: string, body = "Body") =>
  name === null ? body : `---\nname: ${name}\ndescription: ${description}\n---\n${body}\n`;

async function write(path: string, content: string) {
  await mkdir(join(path, ".."), { recursive: true });
  await writeFile(path, content);
}

/** 在 root 下造 home + workspace 两棵树。 */
async function build(root: string) {
  const home = join(root, "home");
  const ws = join(root, "ws");
  const sub = join(ws, "sub");
  await mkdir(join(ws, ".git"), { recursive: true });
  await mkdir(sub, { recursive: true });
  const skills = join(ws, ".escode", "skills");
  await write(join(skills, "alpha", "SKILL.md"), md("alpha", "Alpha skill"));
  await write(
    join(skills, "alpha", "_meta.json"),
    JSON.stringify({ slug: " s ", version: "1", publishedAt: 5, junk: 1 }),
  );
  await write(join(skills, "group", "beta", "SKILL.md"), md("beta", "Beta in a group"));
  await write(join(skills, "node_modules", "x", "SKILL.md"), md("excluded", "never listed"));
  await write(join(skills, ".hidden", "SKILL.md"), md("hidden", "dot dir"));
  await write(join(skills, ".system", "sys", "SKILL.md"), md("sys", "allowed dot dir"));
  await write(join(skills, "nofront", "SKILL.md"), "Just text, no frontmatter");
  await write(join(skills, "toolong", "SKILL.md"), md("toolong", "x".repeat(1025)));
  await write(join(skills, "emptyname", "SKILL.md"), "---\nname:\ndescription: d\n---\nbody");
  await write(join(ws, ".agents", "skills", "gamma", "SKILL.md"), md("gamma", "From agents"));
  await write(join(ws, ".escode", "skills", "zeta", "SKILL.md"), md("zeta", "Zeta"));
  await write(join(home, ".escode", "skills", "delta", "SKILL.md"), md("delta", "User delta"));
  await write(
    join(home, ".agents", "skills", "delta", "SKILL.md"),
    md("delta", "Covered by escode"),
  );
  await write(
    join(home, ".agents", "skills", "renamed-dir", "SKILL.md"),
    md("Delta", "Covered by name key"),
  );
  await write(
    join(home, ".agents", "skills", "epsilon", "SKILL.md"),
    md("epsilon", "User epsilon"),
  );
  await mkdir(join(root, "linked-target", "lsk"), { recursive: true });
  await writeFile(join(root, "linked-target", "lsk", "SKILL.md"), md("linked", "Symlinked skill"));
  await symlink(join(root, "linked-target", "lsk"), join(home, ".escode", "skills", "lsk"));
  // 插件：手动目录 + 官方缓存（默认启用/禁用）+ installed_plugins.json
  const plug = join(root, "plug");
  await write(
    join(plug, ".escode-plugin", "plugin.json"),
    JSON.stringify({ name: "plug", skills: "skills" }),
  );
  await write(join(plug, "skills", "pskill", "SKILL.md"), md("pskill", "Plugin skill"));
  const cli = join(home, ".escode", "cli");
  const official = join(cli, "plugins", "cache", "zcode-plugins-official");
  await write(
    join(official, "pdf", "1.0", ".zcode-plugin", "plugin.json"),
    JSON.stringify({ name: "pdf" }),
  );
  await write(
    join(official, "pdf", "1.0", "skills", "pdfskill", "SKILL.md"),
    md("pdfskill", "Default enabled"),
  );
  await write(
    join(official, "other", "1.0", ".zcode-plugin", "plugin.json"),
    JSON.stringify({ name: "other" }),
  );
  await write(
    join(official, "other", "1.0", "skills", "otherskill", "SKILL.md"),
    md("otherskill", "Default disabled"),
  );
  const installed = join(root, "installed");
  await write(
    join(installed, ".claude-plugin", "plugin.json"),
    JSON.stringify({ name: "inst", skills: ["skills", "../escape", "/abs"] }),
  );
  await write(join(installed, "skills", "iskill", "SKILL.md"), md("iskill", "Installed"));
  await write(join(root, "escape", "SKILL.md"), md("escaped", "must not load"));
  await write(
    join(cli, "plugins", "installed_plugins.json"),
    JSON.stringify({
      plugins: [
        { id: "inst@market", marketplace: "market", installPath: installed },
        { id: "", marketplace: "m", installPath: "/x" },
      ],
    }),
  );
  await write(
    join(cli, "config.json"),
    JSON.stringify(
      {
        keep: "me",
        plugins: {
          dirs: [plug],
          enabledPlugins: { "other@zcode-plugins-official": true, "inst@market": true },
        },
      },
      null,
      2,
    ),
  );
  return { home, ws, sub };
}

const text = (value: unknown) => JSON.stringify(value ?? null);
const strip = (value: unknown, root: string) =>
  JSON.parse(
    text(value)
      .split(root)
      .join("$FIXTURE_ROOT")
      .replace(/(glm:[a-z]+:[^:"]+:)[0-9a-f]{12}/g, "$1<hash>"),
  );
async function outcome<T>(run: () => Promise<T>) {
  try {
    return { ok: true as const, value: await run() };
  } catch (error) {
    return { ok: false as const, message: (error as Error).message };
  }
}
async function tree(root: string): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  const walk = async (dir: string) => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isSymbolicLink()) out[path.slice(root.length)] = "<symlink>";
      else if (entry.isDirectory()) await walk(path);
      else out[path.slice(root.length)] = await readFile(path, "utf8");
    }
  };
  await walk(root);
  return out;
}

const base = await realpath(await mkdtemp(join(tmpdir(), "rusthost-skills-golden-")));
const root = join(base, "a");
const fa = await build(root);
const previous = { HOME: process.env.HOME, LABEL: process.env.ESCODE_PROCESS_LABEL };
process.env.HOME = fa.home;
process.env.ESCODE_PROCESS_LABEL = "desktop";

try {
  const { createSkillsService } = await import("../packages/services/src/node.ts");
  const ts = createSkillsService({ isDesktopRuntime: true });

  const lists = [];
  for (const [label, workspacePath] of [
    ["ws", fa.ws],
    ["sub", fa.sub],
  ] as const) {
    lists.push({
      workspace: label,
      result: strip(await ts.list({ workspacePath }), root),
    });
  }
  const names = ((await ts.list({ workspacePath: fa.sub })).skills as Array<{ name: string }>).map(
    (s) => s.name,
  );
  const idOf = async (service: typeof ts, ws: string, name: string, scope?: string) => {
    const listed = await service.list({ workspacePath: ws });
    return (
      (listed.skills as Array<{ id: string; name: string; scope: string }>).find(
        (s) => s.name === name && (!scope || s.scope === scope),
      )?.id ?? "missing"
    );
  };
  type Step = [string, (s: typeof ts, f: typeof fa) => Promise<unknown>];
  const steps: Step[] = [
    [
      "disable alpha",
      async (s, f) =>
        s.setEnabled({
          workspacePath: f.sub,
          skillId: await idOf(s, f.sub, "alpha"),
          enabled: false,
        } as never),
    ],
    ["list after disable", (s, f) => s.list({ workspacePath: f.sub })],
    [
      "disable gamma",
      async (s, f) =>
        s.setEnabled({
          workspacePath: f.sub,
          skillId: await idOf(s, f.sub, "gamma"),
          enabled: false,
        } as never),
    ],
    [
      "re-enable alpha",
      async (s, f) =>
        s.setEnabled({
          workspacePath: f.sub,
          skillId: await idOf(s, f.sub, "alpha"),
          enabled: true,
        } as never),
    ],
    [
      "unknown skill",
      (s, f) =>
        s.setEnabled({
          workspacePath: f.sub,
          skillId: "glm:workspace:nope:000000000000",
          enabled: false,
        } as never),
    ],
    [
      "prompt context",
      (s, f) =>
        s.buildPromptContext({
          workspacePath: f.sub,
          prompt: "please use $beta and $gamma, not $alpha-nope",
        } as never),
    ],
    [
      "prompt context with no match",
      (s, f) => s.buildPromptContext({ workspacePath: f.sub, prompt: "$nothing here" } as never),
    ],
    [
      "copy workspace skill to common",
      async (s, f) =>
        s.copyToCommon({ workspacePath: f.sub, skillId: await idOf(s, f.sub, "gamma") } as never),
    ],
    [
      "copy again conflicts",
      async (s, f) =>
        s.copyToCommon({ workspacePath: f.sub, skillId: await idOf(s, f.sub, "gamma") } as never),
    ],
    [
      "remove from common rejects non-common",
      async (s, f) =>
        s.removeFromCommon({
          workspacePath: f.sub,
          skillId: await idOf(s, f.sub, "iskill"),
        } as never),
    ],
    [
      "remove from common",
      async (s, f) =>
        s.removeFromCommon({
          workspacePath: f.sub,
          skillId: await idOf(s, f.sub, "alpha"),
        } as never),
    ],
    [
      "delete plugin skill rejected",
      async (s, f) =>
        s.deleteSkill({ workspacePath: f.sub, skillId: await idOf(s, f.sub, "pskill") } as never),
    ],
    [
      "delete symlinked user skill",
      async (s, f) =>
        s.deleteSkill({ workspacePath: f.sub, skillId: await idOf(s, f.sub, "linked") } as never),
    ],
    [
      "delete workspace skill",
      async (s, f) =>
        s.deleteSkill({ workspacePath: f.sub, skillId: await idOf(s, f.sub, "beta") } as never),
    ],
    ["final list", (s, f) => s.list({ workspacePath: f.sub })],
  ];

  const recorded: Array<{ label: string; result: unknown; files: unknown }> = [];
  for (const [label, step] of steps) {
    const result = await outcome(() => step(ts, fa));
    const files = await tree(root);
    recorded.push({
      label,
      result: strip(result, root),
      files: strip(
        Object.fromEntries(Object.entries(files).filter(([p]) => !p.endsWith("skills-audit.log"))),
        root,
      ),
    });
  }
  const configText = (await readFile(join(fa.home, ".escode", "cli", "config.json"), "utf8"))
    .split(root)
    .join("$FIXTURE_ROOT");

  const golden = {
    _readme:
      "rustHostSkills.test.ts 的 TS createSkillsService 观察冻结（Phase 3 golden freeze）。夹具根归一化为 $FIXTURE_ROOT，skill id 路径哈希归一化为 <hash>；files 为每步变更后的目录树（排除 skills-audit.log）。",
    names,
    lists,
    steps: recorded,
    configText,
  };

  const goldenDir = "packages/desktop/test/fixtures/golden";
  const goldenPath = join(goldenDir, "rustHostSkills.oracle.json");
  const json = `${JSON.stringify(golden, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    deepEqual(
      JSON.parse(await readFile(goldenPath, "utf8")),
      JSON.parse(json),
      "rustHostSkills golden 与当前 TS 观察漂移",
    );
    console.log("rustHostSkills golden: OK");
  } else {
    await mkdir(goldenDir, { recursive: true });
    await writeFile(goldenPath, json);
    console.log("wrote", goldenPath, "root=", root);
  }
} finally {
  if (previous.HOME === undefined) delete process.env.HOME;
  else process.env.HOME = previous.HOME;
  if (previous.LABEL === undefined) delete process.env.ESCODE_PROCESS_LABEL;
  else process.env.ESCODE_PROCESS_LABEL = previous.LABEL;
  await rm(base, { recursive: true, force: true });
}
