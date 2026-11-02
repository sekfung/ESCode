// 差分运行模式解析。默认只跑 Rust 对冻结 golden，删除 Node 后无需再改默认路径；
// 只有显式 --record（写 golden）或 --live-node（实时双跑演练）才运行 Node 侧。

export function resolveParityMode({ record = false, golden = false, liveNode = false } = {}) {
  const selected = [
    ["record", record],
    ["golden", golden],
    ["live-node", liveNode],
  ].filter(([, enabled]) => enabled);
  if (selected.length > 1) {
    throw new Error(
      `--${selected.map(([name]) => name).join(" 与 --")} 不能同时使用：请只选择一种差分模式`,
    );
  }
  if (record) return "record";
  if (liveNode) return "live";
  return "golden";
}

export function resolveParityModeFromValues(values = {}) {
  return resolveParityMode({
    record: values.record,
    golden: values.golden,
    liveNode: values["live-node"],
  });
}

export function sidesForMode(mode) {
  if (mode === "golden") return ["rust"];
  if (mode === "record" || mode === "live") return ["node", "rust"];
  throw new Error(`未知差分模式 ${mode}`);
}

export function describeMode(mode, scenario, goldenFile) {
  if (mode === "record") {
    return `[parity] 场景 ${scenario}：Node 与 Rust 各跑一次并录制 golden（${goldenFile}）`;
  }
  if (mode === "live") {
    return `[parity] 场景 ${scenario}：显式实时双跑，Node 与 Rust 各跑一次（确定性输入，mock SSE）`;
  }
  return `[parity] 场景 ${scenario}：Rust 跑一次，对 golden 判定（${goldenFile}）`;
}
