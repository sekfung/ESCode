// 差分垂直切片（docs/specs/rust-unified-host.md §4.2 / §4.4，阶段 0）。
//
// 同一场景在 Node bundle 与 Rust 二进制上各跑一次——同一 mock SSE provider、同一临时
// HOME/workspace/storage、同一驱动脚本、同一确定性输入（固定 commandId/clientId/文本）——
// 把未归一的原始事实逐字节对拍，产出机器可读 evidence.json 与人读 report.md：
//   - frames：V4 帧（stdout NDJSON 逐行原文，含序号与 sha256）
//   - modelRequests：mock provider 收到的全部请求体（按序）
//   - workspaceFiles：场景结束后 workspace 的文件清单（相对路径 + sha256）
//   - storageFacts：各侧存储目录的文件级事实（表级/行级对拍待存储跨读轮接入）
//
// 归一化只做显式登记的最小集合（规则随 evidence 写出，见 diff 模块 NORMALIZATION）。差异分两类：
// approved（登记在 scripts/diff-escode-cli-rust-parity.approved.json，必须写 reason 与 removal）
// 与 divergence（真偏移 → 退出码 1，报首个不同 UTF-16 位置、JSON 路径、序号与双侧 sha256）。
// 禁止 trim / 合并 / 重排等"近似"手段（§4.1 禁止清单）。
//
// 用法：
//   node scripts/diff-escode-cli-rust-parity.mjs                       # 默认 session-baseline
//   node scripts/diff-escode-cli-rust-parity.mjs --rust-binary <path> --keep
//   node scripts/diff-escode-cli-rust-parity.mjs --record              # 冻结 Node 侧观察为 golden
//   node scripts/diff-escode-cli-rust-parity.mjs --golden              # 只跑 Rust，对 golden 判定
//
// golden freeze（阶段 3）：`--record` 把 Node 侧的归一化观察写进 `scripts/fixtures/parity/`；
// `--golden` 不再启动 Node，只用冻结资产作 oracle。删除 Node 之前 CI 先切到 `--golden`。
//
// 依赖产物：`apps/escode-cli/packages/cli/dist/escode.cjs`（scripts/build-desktop-agent-cli.mjs
// 或 CLI 包构建）与 Rust 二进制（cargo build --release）。

import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { runSide } from "./diff-escode-cli-rust-parity-capture.mjs";
import {
  loadGolden,
  recordGolden,
  resolveGoldenFile,
} from "./diff-escode-cli-rust-parity-golden.mjs";
import {
  clip,
  countByteLevelDifferences,
  diffLines,
  fingerprint,
  matchApproved,
  NORMALIZATION,
  normalizeLine,
  sha256,
  textBeforeKeyOrder,
} from "./diff-escode-cli-rust-parity-diff.mjs";
import { writeDiffArtifacts } from "./diff-escode-cli-rust-parity-report.mjs";

const root = resolve(import.meta.dirname, "..");
const { values } = parseArgs({
  options: {
    scenario: { type: "string", default: "session-baseline" },
    out: { type: "string", default: ".escode-runtime/parity" },
    "node-bundle": { type: "string" },
    "rust-binary": { type: "string" },
    approved: { type: "string" },
    baseline: { type: "string" },
    "update-baseline": { type: "boolean", default: false },
    record: { type: "boolean", default: false },
    golden: { type: "boolean", default: false },
    "golden-file": { type: "string" },
    keep: { type: "boolean", default: false },
    strict: { type: "boolean", default: false },
  },
  allowPositionals: false,
});

const nodeBundle = resolve(
  root,
  values["node-bundle"] ?? "apps/escode-cli/packages/cli/dist/escode.cjs",
);
const rustBinary = resolve(
  root,
  values["rust-binary"] ??
    `apps/escode-cli-rust/target/release/escode-cli-rust${process.platform === "win32" ? ".exe" : ""}`,
);
const outRoot = resolve(root, values.out);
const approvedFile = resolve(
  root,
  values.approved ?? "scripts/diff-escode-cli-rust-parity.approved.json",
);
const baselineFile = resolve(
  root,
  values.baseline ?? "scripts/diff-escode-cli-rust-parity.baseline.json",
);
const scenario = values.scenario;
if (scenario !== "session-baseline") throw new Error(`未知场景 ${scenario}`);
if (values.record && values.golden) throw new Error("--record 与 --golden 不能同时使用");
const goldenFile = resolveGoldenFile({
  root,
  scenario,
  explicit: values["golden-file"],
});

const approved = JSON.parse(await readFile(approvedFile, "utf8"));
for (const entry of approved.entries) {
  if (!entry.id || !entry.reason || !entry.removal) {
    throw new Error(`approved 登记表条目缺少 id/reason/removal：${JSON.stringify(entry)}`);
  }
  if (entry.status !== undefined && entry.status !== "approved" && entry.status !== "open") {
    throw new Error(`approved 登记表条目 status 只能是 approved 或 open：${entry.id}`);
  }
}

console.log(
  values.golden
    ? `[parity] 场景 ${scenario}：Rust 跑一次，对 golden 判定（${goldenFile}）`
    : `[parity] 场景 ${scenario}：Node 与 Rust 各跑一次（确定性输入，mock SSE）`,
);
const sides = {};
for (const side of values.golden ? ["rust"] : ["node", "rust"]) {
  sides[side] = await runSide({ side, repoRoot: root, nodeBundle, rustBinary, keep: values.keep });
  console.log(
    `[parity] ${side}: exit ${sides[side].exitCode}，frames ${sides[side].frames.length}，` +
      `modelRequests ${sides[side].modelRequests.length}，startup ${sides[side].startupMs}ms`,
  );
}
for (const side of Object.values(sides)) {
  // 运行期标识符映射按侧独立（同一侧内一致），跨侧错位仍可见。
  const tokenMap = new Map();
  side.framesNormalized = side.frames.map((line) => normalizeLine(line, side.root, tokenMap));
  side.framesByteLevel = side.frames.map((line) => textBeforeKeyOrder(line, side.root));
  side.modelRequestsNormalized = side.modelRequests.map((request) => {
    const row = JSON.stringify({
      method: request.method,
      url: request.url,
      contentType: request.contentType,
      body: JSON.parse(request.body),
    });
    return normalizeLine(row, side.root, tokenMap);
  });
  side.modelRequestsByteLevel = side.modelRequests.map((request) =>
    textBeforeKeyOrder(
      JSON.stringify({
        method: request.method,
        url: request.url,
        contentType: request.contentType,
        body: JSON.parse(request.body),
      }),
      side.root,
    ),
  );
}

if (values.record) {
  await recordGolden({
    file: goldenFile,
    scenario,
    side: sides.node,
    nodeBundleSha256: sha256(await readFile(nodeBundle)),
  });
  console.log(`[parity] golden 已写入：${goldenFile}`);
}

if (values.golden) {
  const loaded = await loadGolden({ file: goldenFile, scenario });
  sides.node = loaded.side;
  console.log(
    `[parity] golden: frames ${sides.node.frames.length}，` +
      `modelRequests ${sides.node.modelRequests.length}，录制于 ${loaded.recordedAt}`,
  );
}

const byteLevelDiffering = {
  frames: countByteLevelDifferences(sides.node.framesByteLevel, sides.rust.framesByteLevel),
  modelRequests: countByteLevelDifferences(
    sides.node.modelRequestsByteLevel,
    sides.rust.modelRequestsByteLevel,
  ),
};

// 计数型流（进度通知/遥测/进程通知）的条数随实现不同，按"方法 × 内嵌 kind"单独成面；
// 其余帧保持严格位置对拍（不允许重排/合并）。
const STREAM_METHODS = new Set([
  "startup/storageState",
  "v4/telemetry/event",
  "process/mcpTelemetry",
  "computer-use/operation-event",
]);
function splitFrames(side) {
  const progress = [];
  const streams = [];
  const messages = [];
  for (const line of side.framesNormalized) {
    let frame = null;
    try {
      frame = JSON.parse(line);
    } catch {
      /* 原始日志行按消息处理。 */
    }
    if (!frame?.method || !STREAM_METHODS.has(frame.method)) {
      messages.push(line);
    } else if (frame.method === "startup/storageState") {
      progress.push(frame);
    } else {
      streams.push({
        method: frame.method,
        kinds: [...line.matchAll(/"kind":"([^"]+)"/g)].map((match) => match[1]),
      });
    }
  }
  return { progress, streams, messages };
}
// 进度/遥测等计数型流的"条数与先后"本身随机抖动（迁移步骤进度、遥测冲刷时机），
// 汇总只比较"出现过哪些阶段/方法/kind"这一集合面，条数与顺序不参与判定（时序事实另记）。
function progressSummary(progress) {
  const keys = {};
  for (const frame of progress) {
    const phase = frame.params?.phase ?? "unknown";
    keys[phase] = Object.keys(frame.params ?? {}).sort();
  }
  return JSON.stringify({ phases: Object.keys(keys).sort(), keys });
}
function streamSummary(streams) {
  return JSON.stringify(
    [
      ...new Set(
        streams.map(
          (entry) => `${entry.method}${entry.kinds.length > 0 ? `:${entry.kinds.join("+")}` : ""}`,
        ),
      ),
    ].sort(),
  );
}
const framesSplit = { node: splitFrames(sides.node), rust: splitFrames(sides.rust) };
const nodeProgress = progressSummary(framesSplit.node.progress);
const rustProgress = progressSummary(framesSplit.rust.progress);
const nodeStreams = streamSummary(framesSplit.node.streams);
const rustStreams = streamSummary(framesSplit.rust.streams);

const summarySurfaces = (name, left, right) =>
  left === right
    ? []
    : [
        {
          surface: name,
          index: 0,
          left,
          right,
          offset: null,
          jsonPath: null,
          leftSha256: sha256(left),
          rightSha256: sha256(right),
        },
      ];
const surfaces = {
  frames: diffLines("frames", framesSplit.node.messages, framesSplit.rust.messages),
  storageProgress: summarySurfaces("storageProgress", nodeProgress, rustProgress),
  streams: summarySurfaces("streams", nodeStreams, rustStreams),
  modelRequests: diffLines(
    "modelRequests",
    sides.node.modelRequestsNormalized,
    sides.rust.modelRequestsNormalized,
  ),
  workspaceFiles: summarySurfaces(
    "workspaceFiles",
    JSON.stringify(sides.node.workspaceFiles),
    JSON.stringify(sides.rust.workspaceFiles),
  ),
};

const divergences = [];
const registeredHits = [];
for (const records of Object.values(surfaces)) {
  for (const record of records) {
    const entry = matchApproved(scenario, record, approved.entries);
    if (entry)
      registeredHits.push({
        ...record,
        registeredBy: entry.id,
        status: entry.status ?? "approved",
      });
    else divergences.push(record);
  }
}
const openOffsets = registeredHits.filter((hit) => hit.status === "open");
const approvedHits = registeredHits.filter((hit) => hit.status !== "open");

// 未分类欠账的基线棘轮：指纹已在基线内 → 已知欠账（不判定失败，列在报告）；新指纹 → 失败。
const baseline = JSON.parse(await readFile(baselineFile, "utf8").catch(() => "{}"));
const baselineSet = new Set(baseline[scenario] ?? []);
const divergeFingerprints = divergences.map((record) => fingerprint(record));
if (values["update-baseline"]) {
  await writeFile(
    baselineFile,
    JSON.stringify(
      {
        ...baseline,
        _readme:
          "未分类欠账基线（scripts/diff-escode-cli-rust-parity.mjs --update-baseline 写入）。指纹已登记的差异不再判失败，但会列在报告里；修复后指纹消失即自动收敛（可再跑 --update-baseline 清理）。",
        [scenario]: [...new Set(divergeFingerprints)].sort(),
      },
      null,
      2,
    ) + "\n",
  );
  console.log(`[parity] 基线已更新：${baselineFile}（${divergeFingerprints.length} 条欠账指纹）`);
  process.exit(0);
}
const knownDebt = divergences.filter((record) => baselineSet.has(fingerprint(record)));
const newDivergences = divergences.filter((record) => !baselineSet.has(fingerprint(record)));

// 时序事实（不参与判定）：两侧在"首轮模型请求"前后各处于哪个阶段，用于分类偏好请求的时序差异。
function timingFacts(side) {
  const titleFirst = side.modelRequests.filter((request) => !request.title);
  const firstTurnRequest = titleFirst[0];
  const prefs = [];
  for (const [index, line] of side.frames.entries()) {
    if (!line.includes("session/requestRuntimePreferences")) continue;
    prefs.push({
      index,
      scope: /"scope":"([^"]+)"/.exec(line)?.[1] ?? "unknown",
      beforeFirstTurnRequest:
        firstTurnRequest?.at !== undefined && side.frameTimes[index] !== undefined
          ? side.frameTimes[index] < firstTurnRequest.at
          : null,
    });
  }
  return { prefs };
}
const timing = { node: timingFacts(sides.node), rust: timingFacts(sides.rust) };

const evidence = {
  scenario,
  generatedAt: new Date().toISOString(),
  platform: `${process.platform}-${process.arch}`,
  inputs: {
    text: "parity turn",
    clientId: "parity",
    commandIds: [1, 2].map((n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`),
    chunksPerTurn: 4,
  },
  artifacts: {
    node: sides.node.golden
      ? { golden: goldenFile, sha256: sha256(await readFile(goldenFile)) }
      : { bundle: nodeBundle, sha256: sha256(await readFile(nodeBundle)) },
    rust: { binary: rustBinary, sha256: sha256(await readFile(rustBinary)) },
  },
  normalization: NORMALIZATION,
  byteLevelDiffering,
  timing,
  sides: Object.fromEntries(
    Object.entries(sides).map(([key, side]) => [
      key,
      {
        exitCode: side.exitCode,
        signal: side.signal,
        startupMs: side.startupMs,
        wallMs: side.wallMs,
        frames: side.frames.length,
        inboundRpcs: side.inboundRpcs,
        modelRequests: side.modelRequests.length,
        workspaceFiles: side.workspaceFiles.length,
        storageFiles: side.storageFacts.inventory.length,
        stderrTail: side.stderrTail,
      },
    ]),
  ),
  surfaces: Object.fromEntries(
    Object.entries(surfaces).map(([name, records]) => {
      const hits = records
        .map((record) => matchApproved(scenario, record, approved.entries))
        .filter(Boolean);
      const unmatched = records.filter(
        (record) => !matchApproved(scenario, record, approved.entries),
      );
      const known = unmatched.filter((record) => baselineSet.has(fingerprint(record)));
      return [
        name,
        {
          differing: records.length,
          approved: hits.filter((entry) => (entry.status ?? "approved") !== "open").length,
          open: hits.filter((entry) => entry.status === "open").length,
          knownDebt: known.length,
          newDivergences: unmatched.length - known.length,
        },
      ];
    }),
  ),
  approvedHits,
  openOffsets,
  knownDebt: {
    count: knownDebt.length,
    bySurface: Object.fromEntries(
      Object.keys(surfaces).map((name) => [
        name,
        knownDebt.filter((record) => record.surface === name).length,
      ]),
    ),
    fingerprints: knownDebt.map((record) => fingerprint(record)),
  },
  newDivergences: newDivergences.map((record) => ({
    ...record,
    left: clip(record.left, record.offset),
    right: clip(record.right, record.offset),
  })),
  storageFacts: { node: sides.node.storageFacts, rust: sides.rust.storageFacts },
  verdict:
    newDivergences.length === 0 &&
    !(values.strict && (openOffsets.length > 0 || knownDebt.length > 0))
      ? "parity"
      : "divergence",
};

await writeDiffArtifacts({ outRoot, scenario, evidence, sides });
console.log(
  `[parity] 判定 ${evidence.verdict}：` +
    Object.entries(evidence.surfaces)
      .map(
        ([name, value]) =>
          `${name} ${value.differing}（批准 ${value.approved}、open ${value.open}、欠账 ${value.knownDebt}、新 ${value.newDivergences}）`,
      )
      .join("；"),
);
console.log(
  `[parity] evidence：${join(outRoot, `${scenario}.evidence.json`)}；报告：${join(outRoot, `${scenario}.report.md`)}`,
);
if (evidence.verdict !== "parity") {
  for (const record of newDivergences.slice(0, 3)) {
    console.error(
      `[parity] 新差异 ${record.surface} #${record.index} @ ${record.jsonPath ?? "-"} offset ${record.offset}`,
    );
    console.error(`  node: ${clip(record.left, record.offset)}`);
    console.error(`  rust: ${clip(record.right, record.offset)}`);
  }
  console.error(
    `[parity] 首次跑通或需要重设欠账基线时运行：node scripts/diff-escode-cli-rust-parity.mjs --update-baseline`,
  );
  process.exit(1);
}
