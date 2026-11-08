// golden freeze（阶段 3）：Node 侧归一化观察已冻结为仓库内只读资产，Rust 侧对 golden 判定。
// Node runtime 退场后录制能力已删除；按平台分文件（system prompt 含 Platform/Shell，跨平台不可共用）。
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

function paths(scenario) {
  return {
    platform: `scripts/fixtures/parity/${scenario}.${process.platform}-${process.arch}.node.golden.json`,
    fallback: `scripts/fixtures/parity/${scenario}.node.golden.json`,
  };
}

/** 显式 --golden-file 优先；否则优先当前平台文件，再回退通用名。 */
export function resolveGoldenFile({ root, scenario, explicit }) {
  if (explicit) return explicit;
  const { platform, fallback } = paths(scenario);
  return resolve(root, existsSync(resolve(root, platform)) ? platform : fallback);
}

export async function loadGolden({ file, scenario }) {
  const golden = JSON.parse(await readFile(file, "utf8"));
  if (golden.scenario !== scenario) {
    throw new Error(`golden 场景不匹配：${golden.scenario} != ${scenario}`);
  }
  return {
    side: {
      ...golden.node,
      root: golden.node.root ?? null,
      golden: true,
      bundleSha256: golden.nodeBundleSha256,
    },
    recordedAt: golden.recordedAt,
  };
}
