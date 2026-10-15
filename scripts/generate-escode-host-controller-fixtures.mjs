// Run with node --import tsx. 用**真实 TS 投影**（packages/desktop/src/host/windowHostControllerProjection.ts）
// 驱动一份操作脚本，导出归一化后的帧序列，供 Rust 侧 window_controller 重放对拍
// （docs/specs/rust-u2-window-controller.md §4.1）。
//
// 归一化：`subscriptionId`（按订阅归类为 `<sub-…>`）/ `logEpoch`（`<logEpoch>`）/ `sentAt`（0）——
// 三者是随机/时钟值（子 spec §5-3 登记）。其余字段（含增量批次边界与水位 fromSeq/toSeq）逐字段对拍。
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import { createWindowHostControllerProjection } from "../packages/desktop/src/host/windowHostControllerProjection.ts";

/** 产物形状变化时递增；Rust 侧加载期断言。 */
const FIXTURE_VERSION = 1;
const OUT = "apps/escode-host/crates/host-rpc/src/window_controller/controller_frames.golden.json";

const WORKSPACE = { kind: "local", workspacePath: "/w", workspaceIdentity: "id-w" };
const TASKS = "controller/tasks-index";
const WORKSPACES = "controller/workspaces";

const ACTIVE = { pinned: false, archived: false, active: true };
const PINNED = { pinned: true, archived: false, active: true };
const ARCHIVED = { pinned: false, archived: true, active: false };

function membership(taskId, updatedAt, flags, meta = {}) {
  return {
    meta: {
      taskId,
      traceId: `trace-${taskId}`,
      title: `title-${taskId}`,
      workspacePath: WORKSPACE.workspacePath,
      workspaceIdentity: WORKSPACE.workspaceIdentity,
      createdAt: updatedAt,
      updatedAt,
      mode: "build",
      ...meta,
    },
    membership: flags,
  };
}

function overlay(taskId, fields) {
  return { taskId, liveStatus: "idle", ...fields };
}

const snapshots = {
  three: [membership("a", 10, ACTIVE), membership("b", 30, PINNED), membership("c", 20, ARCHIVED)],
  withoutC: [membership("a", 10, ACTIVE), membership("b", 40, PINNED)],
  onlyA: [membership("a", 50, ACTIVE)],
};

const overlays = {
  b: [
    overlay("b", {
      liveStatus: "waiting",
      title: "live title b",
      titleSource: "generated",
      updatedAt: 99,
      activity: {
        phase: "running",
        lastActivityAt: 99,
        hasBackgroundWork: true,
        pendingInteractions: { permissionCount: 1, userInputCount: 0 },
      },
      pendingInteraction: { interactionId: "i-1", kind: "permission", toolName: "Bash" },
    }),
  ],
  aDraft: [
    overlay("a", {
      liveStatus: "idle",
      titleSource: "custom",
      title: "renamed a",
      updatedAt: 77,
      activity: { phase: "draft", lastActivityAt: 77, hasBackgroundWork: false },
    }),
  ],
};

/** 脚本即数据：Rust 侧按同一顺序重放（`subscriptionKey` 是归一化后的订阅号占位）。 */
const script = [
  { op: "subscribe", topic: TASKS, subscriptionKey: "sub-tasks" },
  { op: "subscribe", topic: WORKSPACES, subscriptionKey: "sub-workspaces" },
  { op: "registerSource", scope: WORKSPACE },
  { op: "replaceSnapshot", scope: WORKSPACE, memberships: snapshots.three, overlays: overlays.b },
  // 相同输入 → 不发帧（deep-equal 判定）。
  { op: "replaceSnapshot", scope: WORKSPACE, memberships: snapshots.three, overlays: overlays.b },
  // 一行消失 + 一行变化 → removals 先于 upserts。
  { op: "replaceSnapshot", scope: WORKSPACE, memberships: snapshots.withoutC, overlays: [] },
  // overlay 替换只发 upserted（draft 相位让 meta.status 整键省略）。
  { op: "replaceOverlays", scope: WORKSPACE, overlays: overlays.aDraft },
  { op: "replaceOverlays", scope: WORKSPACE, overlays: [] },
  { op: "resync", subscriptionKey: "sub-tasks" },
  // mutation：命中 / 行缺失 / 未知地址（TS 抛错文案与 Rust 逐字对拍）。
  {
    op: "mutate",
    address: { workspacePath: "/w", workspaceIdentity: "id-w", taskId: "a" },
    mutation: { kind: "pin", pinned: true },
  },
  {
    op: "mutate",
    address: { workspacePath: "/w", workspaceIdentity: "id-w", taskId: "missing" },
    mutation: { kind: "pin", pinned: true },
    expectError: "没有与任务地址匹配的 source",
  },
  {
    op: "mutate",
    address: { workspacePath: "/w", taskId: "a" },
    mutation: { kind: "pin", pinned: true },
    expectError: "没有与任务地址匹配的 source",
  },
  // 归档删除允许行缺失（allowMissingTask）。
  {
    op: "mutate",
    address: { workspacePath: "/w", workspaceIdentity: "id-w", taskId: "gone" },
    mutation: { kind: "delete-archived" },
  },
  { op: "unsubscribe", subscriptionKey: "sub-tasks" },
  // 退订后：任务增量不再投递；workspaces 事实变化仍可达 sub-workspaces。
  { op: "replaceSnapshot", scope: WORKSPACE, memberships: snapshots.onlyA, overlays: [] },
  { op: "resync", subscriptionKey: "sub-tasks", expectError: true },
];

async function main() {
  let counter = 0;
  const projection = createWindowHostControllerProjection({
    createId: () => {
      counter += 1;
      return `generated-id-${counter}`;
    },
  });
  const subscriptions = new Map();
  const replacements = new Map();
  const frames = [];
  const errors = [];

  const record = (frame) => frames.push(JSON.parse(JSON.stringify(frame)));

  for (const [index, step] of script.entries()) {
    try {
      switch (step.op) {
        case "subscribe": {
          const subscription = projection.subscribe({ topic: step.topic, onFrame: record });
          subscriptions.set(step.subscriptionKey, subscription);
          replacements.set(subscription.subscriptionId, `<${step.subscriptionKey}>`);
          break;
        }
        case "registerSource":
          projection.registerSource({ scope: step.scope, mutate: async () => null });
          break;
        case "replaceSnapshot":
          projection.replaceSourceSnapshot({
            scope: step.scope,
            taskIndex: step.memberships,
            sessionsIndex: step.overlays,
          });
          break;
        case "replaceOverlays":
          projection.replaceSourceSessionOverlays(step.scope, step.overlays);
          break;
        case "resync": {
          const subscription = subscriptions.get(step.subscriptionKey);
          if (!subscription)
            throw new Error("未找到 Controller subscription，subscriptionId=<gone>");
          projection.resync(subscription.subscriptionId);
          break;
        }
        case "unsubscribe":
          // TS 的 attachment 层持有订阅表；投影侧 dispose 即从 subscribers 摘除。
          subscriptions.get(step.subscriptionKey)?.dispose();
          subscriptions.delete(step.subscriptionKey);
          break;
        case "mutate":
          await projection.mutate(step.address, step.mutation);
          break;
        default:
          throw new Error(`unknown op ${step.op}`);
      }
    } catch (error) {
      if (!step.expectError) throw error;
      errors.push({ index, message: String(error.message ?? error) });
    }
  }

  // logEpoch 是随机值：每个 topic 一个，归一为 `<logEpoch>`（快照内同名字段同步归一）。
  for (const frame of frames) replacements.set(frame.logEpoch, "<logEpoch>");

  const normalize = (value) => {
    if (typeof value === "string") return replacements.get(value) ?? value;
    if (Array.isArray(value)) return value.map(normalize);
    if (value && typeof value === "object") {
      const out = {};
      for (const [key, entry] of Object.entries(value)) {
        out[key] = key === "sentAt" ? 0 : normalize(entry);
      }
      return out;
    }
    return value;
  };

  const fixture = {
    version: FIXTURE_VERSION,
    normalized: ["subscriptionId", "logEpoch", "sentAt"],
    script,
    frames: frames.map(normalize),
    errors,
  };
  const text = `${JSON.stringify(fixture, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    const current = await readFile(OUT, "utf8").catch(() => null);
    if (current !== text) {
      console.error(
        `[controller] 期望资产已过期：${OUT}\n运行 node --import tsx ${process.argv[1]} 重新生成`,
      );
      process.exit(1);
    }
    console.log(`[controller] 期望资产一致：${OUT}`);
    return;
  }
  await mkdir(dirname(OUT), { recursive: true });
  await writeFile(OUT, text);
  process.stdout.write(
    `controller frames golden: ${frames.length} frames, ${errors.length} errors → ${OUT}\n`,
  );
}

await main();
