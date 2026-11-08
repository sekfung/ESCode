import assert from "node:assert/strict";
import { test } from "node:test";

import {
  describeMode,
  resolveParityMode,
  resolveParityModeFromValues,
  sidesForMode,
} from "../diff-escode-cli-rust-parity-mode.mjs";

test("默认模式只跑 Rust，不会隐式启动 Node", () => {
  const mode = resolveParityModeFromValues({});
  assert.equal(mode, "golden");
  assert.deepEqual(sidesForMode(mode), ["rust"]);
  assert.match(describeMode(mode, "session-baseline", "golden.json"), /Rust 跑一次/);
});

test("显式 --golden 仍是 golden-only", () => {
  assert.equal(resolveParityMode({ golden: true }), "golden");
});

test("Node runtime 退场后不存在 record/live 模式", () => {
  assert.equal(resolveParityMode({ record: true }), "golden");
  assert.throws(() => sidesForMode("record"), /Node runtime 已退场/);
  assert.throws(() => sidesForMode("live"), /Node runtime 已退场/);
});
