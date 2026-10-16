// Run with node --import tsx. 用**真实 TS** `readFrontmatter`（packages/services/src/skills/skillsService.ts）
// 对一组 SKILL.md 文本导出解析结果，供 Rust 侧 `skills::frontmatter` 对拍（docs/specs/rust-u2-skills.md §3）。
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import { __INTERNAL__ } from "../packages/services/src/skills/skillsService.ts";
import { DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS } from "../packages/shared/src/plugin-marketplaces.ts";
import {
  MAX_SKILL_SCAN_DEPTH,
  SKILL_FILE_NAME,
  shouldWalkSkillDirectoryEntry,
} from "../packages/shared/src/skill-scan-policy.ts";

const FIXTURE_VERSION = 1;
const OUT = "apps/escode-host/crates/host-services/src/skills/frontmatter_golden.json";
const POLICY_OUT = "apps/escode-host/crates/host-services/src/skills/policy_golden.json";
const WALK_PROBES = [
  "node_modules",
  "dist",
  "build",
  "out",
  "target",
  "vendor",
  "coverage",
  ".cache",
  ".next",
  ".turbo",
  ".venv",
  "__pycache__",
  ".system",
  ".git",
  ".agents",
  "src",
  "skills",
  "Node_Modules",
  "my-skill",
  ".hidden",
];

const docs = [
  "plain markdown without frontmatter\n\nsecond paragraph",
  "---\nname: simple\ndescription: A simple skill\n---\nBody text\n",
  "---\r\nname: crlf\r\ndescription: Windows line endings\r\n---\r\nBody\r\n",
  "---\nname: only-name\n---\n",
  "---\nname: no-close\ndescription: never closed\nBody continues",
  "---\n---\nempty frontmatter",
  '---\nname: quoted\ndescription: "a: b inside quotes"\n---\nbody',
  "---\nname: single\ndescription: 'it''s fine'\n---\nbody",
  "---\nname: 触发\ndescription: 触发场景: 用户提到数据库\n---\n正文",
  "---\nname: unsafe name: here\ndescription: ok\n---\nbody",
  "---\nname: block\ndescription: |\n  line one\n  line two\n---\nbody",
  "---\nname: folded\ndescription: >\n  line one\n  line two\n---\nbody",
  "---\nname: block-unsafe\ndescription: >-\n  a: b\n  c\n---\nbody",
  "---\nname: leaked\ndescription: d\n# Heading Inside\ntext leaked\n---\nreal body",
  "---\nname: leaked-late\ndescription: d\n\n## Usage\nsteps\n---\nafter",
  "---\nname: number\ndescription: 123\n---\nbody",
  "---\nname: 1.50\ndescription: 1e3\n---\nbody",
  "---\nname: true\ndescription: null\n---\nbody",
  "---\nname: ~\ndescription: ~\n---\nbody",
  "---\nname: list\ndescription: [a, b, c]\n---\nbody",
  "---\nname: obj\ndescription: {a: 1}\n---\nbody",
  "---\nname: dup\nname: again\ndescription: d\n---\nbody",
  "---\n- a\n- b\n---\nbody",
  "---\njust a scalar\n---\nbody",
  "---\nname: tabs\n\tdescription: bad indent\n---\nbody",
  "---\nname: [unclosed\ndescription: d\n---\nbody",
  "---\nname: extra\ndescription: d\nlicense: MIT\nmetadata:\n  version: 1\n  tags: [x, y]\nallowed-tools: Read\n---\nbody",
  "---\n# comment first\nname: commented\n  # indented comment\ndescription: d # trailing\n---\nbody",
  '---\nname: "unterminated\ndescription: d\n---\nbody',
  "---\nname:   spaced   \ndescription:   padded desc   \n---\n\n\n  body with spaces  \n\n",
  "---\nname: dashes\ndescription: has --- inside\n---\nbody\n---\nmore",
  "---\nname: first\n---\nname: second\n---\nbody",
  "---\n名称: 中文键\nname: ok\ndescription: d\n---\nbody",
  "---\nname: a\ndescription: |\n    deep indent\n  shallow\n---\nbody",
  "---\nname: a\ndescription: |\n\ttabbed\n---\nbody",
  "---\r\rname: cr only\r---\rbody",
  "﻿---\nname: bom\n---\nbody",
  "---\nname: 'it''s'\ndescription: \"line\\nbreak\"\n---\nbody",
  "---\nname: multi\ndescription: first line\n  continued line\n---\nbody",
  "---\nName: caps\nDescription: Caps\n---\nbody",
  "---\nname: a\ndescription: b\n---",
  "---\nname: a\ndescription: b\n---\n# real heading\ntext",
];

async function emit(path, payload, label) {
  const text = `${JSON.stringify(payload, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    const current = await readFile(path, "utf8").catch(() => null);
    if (current !== text) {
      console.error(
        `[skills] 期望资产已过期：${path}\n运行 node --import tsx ${process.argv[1]} 重新生成`,
      );
      process.exit(1);
    }
    console.log(`[skills] 期望资产一致：${path}`);
    return;
  }
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, text);
  process.stdout.write(`skills ${label} → ${path}\n`);
}

async function main() {
  const cases = docs.map((text) => ({ text, expected: __INTERNAL__.readFrontmatter(text) }));
  await emit(
    OUT,
    { fixtureVersion: FIXTURE_VERSION, cases },
    `frontmatter golden: ${cases.length} cases`,
  );
  await emit(
    POLICY_OUT,
    {
      fixtureVersion: FIXTURE_VERSION,
      skillFileName: SKILL_FILE_NAME,
      maxScanDepth: MAX_SKILL_SCAN_DEPTH,
      defaultEnabledPluginIds: [...DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS],
      walk: WALK_PROBES.map((name) => ({ name, walk: shouldWalkSkillDirectoryEntry(name) })),
    },
    "policy golden",
  );
}

await main();
