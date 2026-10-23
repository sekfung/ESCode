import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// Phase 3 golden freeze：TS `TaskIndexRepo` 的逐步观察已冻结（group id 归一化、时间戳剔除），
// 测试不再实例化 TS Repo；Rust Host 从冻结的 seed 库启动。
const goldenFile = new URL("./fixtures/golden/rustHostTaskGroups.oracle.json", import.meta.url);
const goldenSeed = new URL("./fixtures/golden/rustHostTaskGroups.seed.sqlite", import.meta.url);

// docs/specs/rust-u2-tasks-index.md §6.8 TS oracle 差分：分组视图写面（createTaskGroup / renameTaskGroup /
// updateTaskGroupColor / deleteTaskGroup / applyGroupedTaskViewOrder）。同一脚本分别驱动真实 TS `TaskIndexRepo`
// （库 A）与 Rust Host（库 B，真实 `@escode/client`），每步之后比较分组结构（group id 归一化、时间戳剔除）。

const scopes = [{ workspacePath: "/w" }];
const ref = (taskId: string, workspacePath = "/w") => ({ workspacePath, taskId });

/** 去掉时间戳，把随机 group id 换成稳定标签。 */
function normalize(value: unknown, ids: Map<string, string>): unknown {
  let text = JSON.stringify(value, (key, item) =>
    /^(created|updated|added)At$/.test(key) ? undefined : item,
  );
  if (text === undefined) return "<undefined>";
  for (const [id, label] of ids) text = text.split(id).join(label);
  return JSON.parse(text);
}

type Structure = { topLevelOrders: Array<{ type: string; taskId?: string }> };

/**
 * TS 的 `applyGroupedTaskViewOrder` 回包走旧的 `queryGroupedTaskView`，它会顺带把「当前可见但没有顶层排序」的
 * 任务补成用户排序（隐式写回）。Rust 不复制这条过渡面的副作用（登记差异，Renderer 以 createdAt 派生缺省序）。
 * 这里只允许 TS 一侧多出 task 类型的补齐行，其余字段必须逐项相等。
 */
function reconcile(key: string, expected: unknown, actual: unknown): unknown {
  const wanted = expected as Structure;
  const got = actual as Structure;
  if (!wanted?.topLevelOrders || !got?.topLevelOrders) return expected;
  const have = new Set(got.topLevelOrders.map((order) => `${order.type}:${order.taskId ?? ""}`));
  const extras = wanted.topLevelOrders.filter(
    (order) => !have.has(`${order.type}:${order.taskId ?? ""}`),
  );
  expect(
    extras.every((order) => order.type === "task"),
    `${key}: 多出的只能是 task 补齐行`,
  ).toBe(true);
  return {
    ...wanted,
    topLevelOrders: wanted.topLevelOrders.filter((order) => !extras.includes(order)),
  };
}

interface Driver {
  create(params: { title?: string; color?: string }): Promise<{ id: string }>;
  rename(groupId: string, title: string): Promise<unknown>;
  recolor(groupId: string, color: string): Promise<unknown>;
  remove(groupId: string): Promise<unknown>;
  apply(params: { topLevelNodes: unknown[]; groups: unknown[] }): Promise<unknown>;
  structure(): Promise<unknown>;
}

async function script(driver: Driver): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};
  const ids = new Map<string, string>();
  const label = (group: { id: string }, name: string) => {
    ids.set(group.id, name);
    return group;
  };
  const settle = async (run: () => Promise<unknown>) =>
    run().then(
      (value) => ({ ok: normalize(value, ids) }),
      (error: Error) => ({ error: error.message }),
    );
  const snapshot = async (name: string) => {
    out[name] = normalize(await driver.structure(), ids);
  };

  const g1 = label(await driver.create({ title: "  Alpha ", color: "red" }), "G1");
  const g2 = label(await driver.create({}), "G2");
  out.created = normalize([g1, g2], ids);
  await snapshot("afterCreate");

  out.renameBlank = await settle(() => driver.rename(g1.id, "   "));
  out.recolor = await settle(() => driver.recolor(g2.id, "purple"));
  out.recolorInvalid = await settle(() => driver.recolor(g2.id, "pink"));
  out.renameMissing = await settle(() => driver.rename("task-group-ghost", "x"));
  out.recolorMissing = await settle(() => driver.recolor("task-group-ghost", "red"));
  await snapshot("afterEdits");

  out.apply = await settle(() =>
    driver.apply({
      topLevelNodes: [
        { type: "task", task: ref("t3") },
        { type: "group", groupId: g2.id },
        { type: "group", groupId: g1.id },
      ],
      groups: [
        { groupId: g1.id, taskRefs: [ref("t2"), ref("t1")] },
        { groupId: g2.id, taskRefs: [] },
      ],
    }),
  );
  await snapshot("afterApply");

  // t1 提到顶层（退出 g1），顺序整体重排。
  out.applyMove = await settle(() =>
    driver.apply({
      topLevelNodes: [
        { type: "group", groupId: g1.id },
        { type: "task", task: ref("t1") },
        { type: "group", groupId: g2.id },
      ],
      groups: [],
    }),
  );
  await snapshot("afterMove");

  out.applyDuplicate = await settle(() =>
    driver.apply({
      topLevelNodes: [],
      groups: [
        { groupId: g1.id, taskRefs: [ref("t1")] },
        { groupId: g2.id, taskRefs: [ref("t1")] },
      ],
    }),
  );
  out.applyGhostGroup = await settle(() =>
    driver.apply({ topLevelNodes: [{ type: "group", groupId: "task-group-ghost" }], groups: [] }),
  );
  out.applyOutsideScope = await settle(() =>
    driver.apply({ topLevelNodes: [{ type: "task", task: ref("t1", "/other") }], groups: [] }),
  );
  out.applyArchived = await settle(() =>
    driver.apply({ topLevelNodes: [{ type: "task", task: ref("t_archived") }], groups: [] }),
  );
  out.applyMissing = await settle(() =>
    driver.apply({ topLevelNodes: [{ type: "task", task: ref("nope") }], groups: [] }),
  );
  await snapshot("afterRejected");

  out.remove = await settle(() => driver.remove(g2.id));
  out.removeAgain = await settle(() => driver.remove(g2.id));
  await snapshot("afterRemove");
  return out;
}

describe("rust host task groups", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the TS TaskIndexRepo for the grouped-view write surface",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-groups-")));
      const home = join(base, "home");
      const dataBaseDir = join(base, "data");
      const rustDb = join(dataBaseDir, ".escode", "v2", "tasks-index.sqlite");
      await mkdir(join(dataBaseDir, ".escode", "v2"), { recursive: true });
      await copyFile(goldenSeed, rustDb);
      const golden = JSON.parse(await readFile(goldenFile, "utf8")) as {
        expected: Record<string, unknown>;
      };

      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const task = services.escodeTaskService;

        const events: Array<Record<string, unknown>> = [];
        task.onDynamicWorkspaceEvent("/w")((event) => events.push(event as never));

        const rustDriver: Driver = {
          create: (params) => task.createTaskGroup(params as never) as never,
          rename: (groupId, title) =>
            task.renameTaskGroup({ groupId, title, workspaceScopes: scopes } as never),
          recolor: (groupId, color) =>
            task.updateTaskGroupColor({ groupId, color, workspaceScopes: scopes } as never),
          remove: (groupId) => task.deleteTaskGroup({ groupId, workspaceScopes: scopes } as never),
          apply: (params) =>
            task.applyGroupedTaskViewOrder({ workspaceScopes: scopes, ...params } as never),
          structure: () => task.listGroupedTaskViewStructure({ workspaceScopes: scopes } as never),
        };

        const expected = golden.expected;
        const actual = await script(rustDriver);
        for (const key of Object.keys(expected)) {
          if (key === "apply" || key === "applyMove") continue; // 回包 nodes 为登记差异，见下
          expect({ key, value: reconcile(key, expected[key], actual[key]) }).toEqual({
            key,
            value: actual[key],
          });
        }
        expect(Object.keys(actual)).toEqual(Object.keys(expected));
        // apply 回包：写入成功/失败判定一致；成功时 Rust 的 nodes 恒空（登记差异）。
        expect(actual.apply).toEqual({ ok: { nodes: [] } });
        expect((expected.apply as { error?: string }).error).toBeUndefined();
        expect(actual.applyMove).toEqual({ ok: { nodes: [] } });

        // 事件：rename/recolor(成功)/apply×2/remove 成功各广播一次 task_meta_changed，创建与失败调用不广播。
        await new Promise((resolve) => setTimeout(resolve, 200));
        const reasons = events.map((event) => event.reason);
        expect(reasons.filter((reason) => reason === "task_meta_changed").length).toBe(5);
        expect(
          events.every((event) => event.taskMeta === undefined && event.taskId === undefined),
        ).toBe(true);
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
