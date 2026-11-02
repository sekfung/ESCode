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

test("--record 运行双侧并写 golden", () => {
  assert.equal(resolveParityMode({ record: true }), "record");
  assert.deepEqual(sidesForMode("record"), ["node", "rust"]);
});

test("--live-node 显式运行双侧但不写 golden", () => {
  assert.equal(resolveParityMode({ liveNode: true }), "live");
  assert.deepEqual(sidesForMode("live"), ["node", "rust"]);
});

test("互斥模式直接失败，避免误用覆盖 golden", () => {
  assert.throws(
    () => resolveParityMode({ record: true, liveNode: true }),
    /--record 与 --live-node 不能同时使用/,
  );
  assert.throws(
    () => resolveParityMode({ golden: true, liveNode: true }),
    /--golden 与 --live-node 不能同时使用/,
  );
});
