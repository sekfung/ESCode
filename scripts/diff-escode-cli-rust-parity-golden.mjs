// golden freeze（阶段 3）：把 Node 侧归一化观察冻结为仓库内资产，并让 Rust 侧脱离 Node 判定。
// 按平台分文件：system prompt 含 Platform/Shell，跨平台不可共用同一份观察。
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

function paths(scenario) {
  return {
    platform: `scripts/fixtures/parity/${scenario}.${process.platform}-${process.arch}.node.golden.json`,
    fallback: `scripts/fixtures/parity/${scenario}.node.golden.json`,
  };
}

/**
 * 显式 --golden-file 优先；--record 必须写当前平台文件（否则首次录制会落到通用名，
 * 平台门禁永远不出现）；--golden 优先当前平台文件，再回退通用名。
 */
export function resolveGoldenFile({ root, scenario, explicit, record = false }) {
  if (explicit) return explicit;
  const { platform, fallback } = paths(scenario);
  if (record) return resolve(root, platform);
  return resolve(root, existsSync(resolve(root, platform)) ? platform : fallback);
}

export function goldenPayload(side) {
  return {
    // 录制侧临时根：--golden 重新归一 raw frames 时用它替换 <ROOT>。
    root: side.root,
    exitCode: side.exitCode,
    signal: side.signal,
    startupMs: side.startupMs,
    wallMs: side.wallMs,
    inboundRpcs: side.inboundRpcs,
    stderrTail: side.stderrTail,
    frames: side.frames,
    frameTimes: side.frameTimes,
    framesNormalized: side.framesNormalized,
    framesByteLevel: side.framesByteLevel,
    modelRequests: side.modelRequests,
    modelRequestsNormalized: side.modelRequestsNormalized,
    modelRequestsByteLevel: side.modelRequestsByteLevel,
    workspaceFiles: side.workspaceFiles,
    storageFacts: side.storageFacts,
  };
}

export async function recordGolden({ file, scenario, side, nodeBundleSha256 }) {
  await mkdir(dirname(file), { recursive: true });
  await writeFile(
    file,
    `${JSON.stringify(
      {
        _readme:
          "Node 侧冻结观察（scripts/diff-escode-cli-rust-parity.mjs --record 写入）。归一化规则随证据输出；" +
          "删除 Node 后 `--golden` 用它作唯一 oracle。重新录制必须是一次已通过实时双跑的运行。",
        scenario,
        recordedAt: new Date().toISOString(),
        platform: `${process.platform}-${process.arch}`,
        nodeBundleSha256,
        node: goldenPayload(side),
      },
      null,
      2,
    )}\n`,
  );
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
