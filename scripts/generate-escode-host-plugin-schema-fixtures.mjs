// Run with node --import tsx. 用**真实 zod schema**（packages/shared/src/escode-protocol）对一组有效样本及其系统化变异
// 求 `safeParse().success`，供 Rust 侧 `plugin_result_schema` 对拍（docs/specs/rust-u2-plugin-management.md §7）。
// 只导出接受/拒绝，不导出 zod 的错误文案。样本在 escode-host-plugin-schema-samples.mjs。
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { isDeepStrictEqual } from "node:util";

import { NORMALIZED_SCHEMAS, SAMPLES } from "./escode-host-plugin-schema-samples.mjs";

const FIXTURE_VERSION = 1;
const OUT = "apps/escode-host/crates/host-rpc/src/plugin_schema_golden.json";

// 变异替换值：类型错、边界、JS 与 Rust `trim` 不一致的空白、安全整数边界。
const REPLACEMENTS = [
  null,
  true,
  false,
  0,
  1,
  -1,
  1.5,
  1.0,
  9007199254740991,
  9007199254740992,
  "",
  "  ",
  "x",
  "﻿",
  "\u0085",
  " ",
  " ",
  "​",
  " \t\n ",
  "user",
  "missing",
  [],
  {},
  [""],
  { a: 1 },
];

const clone = (value) => JSON.parse(JSON.stringify(value));

function setAt(root, path, value) {
  const copy = clone(root);
  let cursor = copy;
  for (const key of path.slice(0, -1)) cursor = cursor[key];
  cursor[path[path.length - 1]] = value;
  return copy;
}
function deleteAt(root, path) {
  const copy = clone(root);
  let cursor = copy;
  for (const key of path.slice(0, -1)) cursor = cursor[key];
  delete cursor[path[path.length - 1]];
  return copy;
}
function addKeyAt(root, path) {
  const copy = clone(root);
  let cursor = copy;
  for (const key of path) cursor = cursor[key];
  cursor.__extra = 1;
  return copy;
}

function* paths(value, prefix = []) {
  yield prefix;
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i += 1) yield* paths(value[i], [...prefix, i]);
  } else if (value !== null && typeof value === "object") {
    for (const key of Object.keys(value)) yield* paths(value[key], [...prefix, key]);
  }
}
const at = (root, path) => path.reduce((cursor, key) => cursor[key], root);

// 配方：[op, path, replacementIndex]；op 0 = 原样，1 = 给 path 处对象加未知键，2 = 删 path 处的键，3 = 把 path 处换成 REPLACEMENTS[i]。
// 只导出配方而不是变异后的完整值：Rust 侧用同一套 apply 重放，资产体积小一个数量级。
function* recipes(base) {
  yield [0, [], 0];
  for (const path of paths(base)) {
    const node = at(base, path);
    if (node !== null && typeof node === "object" && !Array.isArray(node)) yield [1, path, 0];
    if (path.length === 0) continue;
    const parent = at(base, path.slice(0, -1));
    if (!Array.isArray(parent)) yield [2, path, 0];
    for (let i = 0; i < REPLACEMENTS.length; i += 1) yield [3, path, i];
  }
}

function applyRecipe(base, [op, path, index]) {
  if (op === 0) return base;
  if (op === 1) return addKeyAt(base, path);
  if (op === 2) return deleteAt(base, path);
  return setAt(base, path, REPLACEMENTS[index]);
}

async function build() {
  const bases = [];
  const cases = [];
  const seen = new Set();
  let accepted = 0;
  for (const { schema, zod, values } of SAMPLES) {
    for (const value of values) {
      const baseIndex = bases.push({ schema, value }) - 1;
      for (const recipe of recipes(value)) {
        const mutated = applyRecipe(value, recipe);
        const key = `${schema}\u0000${JSON.stringify(mutated)}`;
        if (seen.has(key)) continue;
        seen.add(key);
        const parsed = zod.safeParse(mutated);
        const ok = parsed.success;
        if (ok) accepted += 1;
        // spec §7 的论断：除 NORMALIZED_SCHEMAS 外，接受的值经 `parse` 后与输入逐值相同（纯接受器，Rust 才能原样透传）。
        if (ok && !NORMALIZED_SCHEMAS.has(schema) && !isDeepStrictEqual(parsed.data, mutated)) {
          throw new Error(`${schema}: parse output differs from input; not a pure acceptor`);
        }
        cases.push([baseIndex, recipe[0], recipe[1], recipe[2], ok]);
      }
    }
  }
  if (accepted === 0 || accepted === cases.length) {
    throw new Error("variation corpus must contain both accepted and rejected cases");
  }
  const normalizations = [];
  for (const { schema, zod, values } of SAMPLES) {
    if (!NORMALIZED_SCHEMAS.has(schema)) continue;
    const mixed = clone(values[0]);
    mixed.plugins.push({ ...clone(values[1].plugins[0]) });
    for (const input of [...values, mixed]) {
      normalizations.push({ schema, input, output: zod.parse(input) });
    }
  }
  return {
    fixtureVersion: FIXTURE_VERSION,
    replacements: REPLACEMENTS,
    bases,
    cases,
    normalizations,
  };
}

async function main() {
  const payload = await build();
  const text = `${JSON.stringify(payload)}\n`;
  if (process.argv.includes("--check")) {
    const current = await readFile(OUT, "utf8").catch(() => null);
    if (current !== text) {
      console.error(
        `[plugin-schema] 期望资产已过期：${OUT}\n运行 node --import tsx ${process.argv[1]} 重新生成`,
      );
      process.exit(1);
    }
    console.log(`[plugin-schema] 期望资产一致：${OUT}`);
    return;
  }
  await mkdir(dirname(OUT), { recursive: true });
  await writeFile(OUT, text);
  const accepted = payload.cases.filter((entry) => entry[4]).length;
  process.stdout.write(
    `plugin schema golden: ${payload.cases.length} cases (${accepted} accepted) → ${OUT}\n`,
  );
}

await main();
