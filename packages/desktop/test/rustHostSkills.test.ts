import { describe, expect, it, vi } from "vitest";
import { existsSync } from "node:fs";
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
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-skills.md：skills 服务对真实 TS 实现差分（同一棵目录树上 list 精确相等；变更操作在两棵同构树上比较归一化结果）。

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
      .join("<root>")
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

describe("rust host skills", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the TS skills service",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-skills-")));
      const a = join(base, "a");
      const b = join(base, "b");
      const fa = await build(a);
      const fb = await build(b);
      const previous = { HOME: process.env.HOME, LABEL: process.env.ESCODE_PROCESS_LABEL };
      process.env.HOME = fa.home;
      process.env.ESCODE_PROCESS_LABEL = "desktop";
      vi.resetModules();
      const { createSkillsService } = await import("@escode/services/node");
      const ts = createSkillsService({ isDesktopRuntime: true });
      const start = async (home: string) => {
        const host = startHost(home, { ESCODE_PROCESS_LABEL: "desktop" });
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir: home });
        const listening = await nextMeaningful(host);
        return { host, rpc: await connectRpc(String(listening.addr), String(listening.secret)) };
      };
      const same = await start(fa.home); // 与 TS 读同一棵树：list 精确相等（含 id 哈希）
      const mirror = await start(fb.home); // 同构的另一棵树：变更操作
      try {
        // ── 同一棵树上的精确相等 ──
        for (const workspacePath of [fa.ws, fa.sub]) {
          const expected = await ts.list({ workspacePath });
          const actual = await same.rpc.skillsService.list({ workspacePath } as never);
          expect({ workspacePath, result: JSON.parse(text(actual)) }).toEqual({
            workspacePath,
            result: JSON.parse(text(expected)),
          });
        }
        const names = (
          (await ts.list({ workspacePath: fa.sub })).skills as Array<{ name: string }>
        ).map((s) => s.name);
        expect(names).toEqual(
          expect.arrayContaining([
            "alpha",
            "beta",
            "gamma",
            "pskill",
            "pdfskill",
            "otherskill",
            "iskill",
            "linked",
            "sys",
          ]),
        );
        expect(names).not.toEqual(expect.arrayContaining(["excluded", "hidden", "escaped"]));

        // ── 变更操作（两棵同构树）──
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
            (s, f) =>
              s.buildPromptContext({ workspacePath: f.sub, prompt: "$nothing here" } as never),
          ],
          [
            "copy workspace skill to common",
            async (s, f) =>
              s.copyToCommon({
                workspacePath: f.sub,
                skillId: await idOf(s, f.sub, "gamma"),
              } as never),
          ],
          [
            "copy again conflicts",
            async (s, f) =>
              s.copyToCommon({
                workspacePath: f.sub,
                skillId: await idOf(s, f.sub, "gamma"),
              } as never),
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
              s.deleteSkill({
                workspacePath: f.sub,
                skillId: await idOf(s, f.sub, "pskill"),
              } as never),
          ],
          [
            "delete symlinked user skill",
            async (s, f) =>
              s.deleteSkill({
                workspacePath: f.sub,
                skillId: await idOf(s, f.sub, "linked"),
              } as never),
          ],
          [
            "delete workspace skill",
            async (s, f) =>
              s.deleteSkill({
                workspacePath: f.sub,
                skillId: await idOf(s, f.sub, "beta"),
              } as never),
          ],
          ["final list", (s, f) => s.list({ workspacePath: f.sub })],
        ];
        for (const [label, step] of steps) {
          const expected = await outcome(() => step(ts, fa));
          const actual = await outcome(() => step(mirror.rpc.skillsService as never, fb));
          // 审计日志中的时间戳不可比；其余逐项比较。
          expect({ label, result: strip(actual, b) }).toEqual({
            label,
            result: strip(expected, a),
          });
          const normTree = async (root: string) => {
            const t = await tree(root);
            return strip(
              Object.fromEntries(
                Object.entries(t).filter(([p]) => !p.endsWith("skills-audit.log")),
              ),
              root,
            );
          };
          expect({ label, files: await normTree(b) }).toEqual({ label, files: await normTree(a) });
        }
        // 变更后的配置文件键序/内容：TS 与 Rust 完全一致
        expect(await readFile(join(fb.home, ".escode", "cli", "config.json"), "utf8")).toBe(
          (await readFile(join(fa.home, ".escode", "cli", "config.json"), "utf8")).split(a).join(b),
        );
      } finally {
        if (previous.HOME === undefined) delete process.env.HOME;
        else process.env.HOME = previous.HOME;
        if (previous.LABEL === undefined) delete process.env.ESCODE_PROCESS_LABEL;
        else process.env.ESCODE_PROCESS_LABEL = previous.LABEL;
        await same.host.stop();
        await mirror.host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    180_000,
  );
});
