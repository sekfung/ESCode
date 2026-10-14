// 差分垂直切片的落盘层：evidence.json、双侧原始/归一帧、模型请求、人读 report.md。

import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { clip, NORMALIZATION } from "./diff-escode-cli-rust-parity-diff.mjs";

function renderReport(scenario, evidence) {
  const { surfaces, knownDebt, newDivergences, openOffsets, approvedHits, timing } = evidence;
  return [
    `# 差分报告：${scenario}`,
    "",
    `- 判定：**${evidence.verdict === "parity" ? "无新差异（含已批准差异与已知欠账）" : `发现 ${newDivergences.length} 处新差异`}**` +
      `；已知欠账（基线内）${knownDebt.count} 处、已登记 open 偏移 ${openOffsets.length} 处（default 不判失败，--strict 判失败）`,
    `- 平台：${evidence.platform}；Node bundle sha256 \`${evidence.artifacts.node.sha256.slice(0, 16)}…\`；Rust binary sha256 \`${evidence.artifacts.rust.sha256.slice(0, 16)}…\``,
    `- 归一化规则：${NORMALIZATION.map((rule) => rule.id).join(" / ")}（明细见 evidence.json；新增规则需 review）`,
    `- 字节级（键序归一前）差异行数：frames ${evidence.byteLevelDiffering.frames}、modelRequests ${evidence.byteLevelDiffering.modelRequests}（诊断项，不参与判定）`,
    `- 时序事实：prefs 请求先于首轮模型请求——node ${timing.node.prefs.map((p) => `${p.scope}:${p.beforeFirstTurnRequest}`).join(" ") || "-"}；rust ${timing.rust.prefs.map((p) => `${p.scope}:${p.beforeFirstTurnRequest}`).join(" ") || "-"}`,
    "",
    "| 面 | 差异行 | 已批准 | 已登记 open | 已知欠账 | 新差异 |",
    "| --- | --- | --- | --- | --- | --- |",
    ...Object.entries(surfaces).map(([name, value]) =>
      [
        name,
        value.differing,
        value.approved,
        openOffsets.filter((hit) => hit.surface === name).length,
        knownDebt.bySurface[name] ?? 0,
        newDivergences.filter((record) => record.surface === name).length,
      ]
        .join(" | ")
        .replace(/^/, "| ")
        .replace(/$/, " |"),
    ),
    "",
    ...(newDivergences.length === 0
      ? ["未发现新差异。", ""]
      : [
          "## 新差异（首个 10 条）",
          "",
          ...newDivergences.slice(0, 10).flatMap((record) => [
            `### ${record.surface} #${record.index}${record.jsonPath ? ` @ ${record.jsonPath}` : ""}`,
            "",
            `- 首个不同位置（UTF-16）：${record.offset}`,
            `- Node：\`${clip(record.left, record.offset)}\``,
            `- Rust：\`${clip(record.right, record.offset)}\``,
            "",
            "```json",
            JSON.stringify(
              {
                id: "TODO",
                scenario,
                surface: record.surface,
                match: record.jsonPath ? { jsonPath: record.jsonPath } : { index: record.index },
                reason: "TODO：确认为真偏移后修复；若为已批准差异，写明理由",
                removal: "TODO",
              },
              null,
              2,
            ),
            "```",
            "",
          ]),
        ]),
    ...(knownDebt.count > 0
      ? [
          "## 已知欠账（基线内，待逐条分类）",
          "",
          `共 ${knownDebt.count} 条；指纹见 evidence.json 的 knownDebt 与 scripts/diff-escode-cli-rust-parity.baseline.json。`,
          "",
        ]
      : []),
    ...(approvedHits.length > 0
      ? [
          "## 已批准差异",
          "",
          ...approvedHits
            .slice(0, 20)
            .map(
              (hit) => `- ${hit.surface} #${hit.index}（${hit.approvedBy}）${hit.jsonPath ?? ""}`,
            ),
          "",
        ]
      : []),
    ...(openOffsets.length > 0
      ? [
          "## 已登记未修复偏移（open）",
          "",
          ...openOffsets.map((hit) => `- ${hit.surface} #${hit.index}（${hit.registeredBy}）`),
          "",
        ]
      : []),
  ].join("\n");
}

/** 写出 evidence.json、双侧原始/归一帧与模型请求、report.md。 */
export async function writeDiffArtifacts({ outRoot, scenario, evidence, sides }) {
  await mkdir(outRoot, { recursive: true });
  await writeFile(join(outRoot, `${scenario}.evidence.json`), JSON.stringify(evidence, null, 2));
  for (const [key, side] of Object.entries(sides)) {
    await writeFile(
      join(outRoot, `${scenario}.${key}.frames.raw.ndjson`),
      side.frames.join("\n") + "\n",
    );
    await writeFile(
      join(outRoot, `${scenario}.${key}.frames.ndjson`),
      side.framesNormalized.join("\n") + "\n",
    );
    await writeFile(
      join(outRoot, `${scenario}.${key}.modelRequests.ndjson`),
      side.modelRequestsNormalized.join("\n") + "\n",
    );
  }
  await writeFile(join(outRoot, `${scenario}.report.md`), renderReport(scenario, evidence));
}
