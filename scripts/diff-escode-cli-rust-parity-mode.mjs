// 差分运行模式解析。Node runtime 退场后只有 golden 模式：Rust 跑一次，对冻结 golden 判定。
// 历史 `--record` / `--live-node` 已删除，任何调用都回到 golden-only。

export function resolveParityMode({ golden = false } = {}) {
  return "golden";
}

export function resolveParityModeFromValues(_values = {}) {
  return "golden";
}

export function sidesForMode(mode) {
  if (mode !== "golden") {
    throw new Error(`Node runtime 已退场，未知差分模式 ${mode}`);
  }
  return ["rust"];
}

export function describeMode(_mode, scenario, goldenFile) {
  return `[parity] 场景 ${scenario}：Rust 跑一次，对 golden 判定（${goldenFile}）`;
}
