// automation 域的真实客户端 golden 差分（docs/specs/rust-u2-automation.md §6）。
//
// Phase 3 golden freeze：原用例在同一 tasks-index 库上交替驱动 TS `AutomationService` 与 Rust Host。
// 现在测试不再实例化 TS 服务：Rust Host 从冻结的 seed 库启动（内含 TS 创建行），每一步的写操作
// 仍走真实 `@escode/client`，读回结果与冻结的 TS 观察逐字段比较。
import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

const goldenFile = new URL(
  "./fixtures/golden/rustHostAutomationParity.oracle.json",
  import.meta.url,
);
const goldenSeed = new URL(
  "./fixtures/golden/rustHostAutomationParity.seed.sqlite",
  import.meta.url,
);

const WORKSPACE_PATH = "/tmp/escode-automation-parity-oracle";
const scope = { workspacePath: WORKSPACE_PATH, workspaceIdentity: WORKSPACE_PATH };
const RUST_LABEL = "RUST_CREATED";

type AutomationRow = Record<string, unknown>;

interface Golden {
  expected: {
    tsCreated: AutomationRow;
    afterTsCreate: AutomationRow[];
    rustCreated: AutomationRow;
    afterRustCreate: AutomationRow[];
    pausedFields: { enabled: boolean; lifecycleStatus: string };
    pausedRow: AutomationRow;
    afterPause: AutomationRow[];
    renamedTitle: string;
    afterRename: AutomationRow[];
    afterDelete: AutomationRow[];
  };
}

/** 与生成脚本一致：剔除时间戳；把本次运行动态生成的 automation id 换成稳定标签。 */
function normalize(value: unknown, labels: Map<string, string>): unknown {
  let text = JSON.stringify(value, (key, item) =>
    /^(createdAt|updatedAt|addedAt|anchorAt|nextRunAt|lastRunAt)$/.test(key) ? undefined : item,
  );
  if (text === undefined) return "<undefined>";
  for (const [id, label] of labels) text = text.split(id).join(label);
  return JSON.parse(text);
}

describe("rust host automation parity", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the frozen TypeScript automation observations in both directions",
    { timeout: 60_000 },
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-automation-parity-"));
      const home = join(root, "home");
      const dataBaseDir = join(root, "data-base");
      await mkdir(home, { recursive: true });
      // 冻结的 TS tasks-index seed：Rust Host 直接读同一文件，测试内不再跑 TS 服务造数。
      const dbDir = join(dataBaseDir, ".escode", "v2");
      await mkdir(dbDir, { recursive: true });
      await copyFile(goldenSeed, join(dbDir, "tasks-index.sqlite"));
      const golden = JSON.parse(await readFile(goldenFile, "utf8")) as Golden;
      const expected = golden.expected;

      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const service = services.escodeAgentService;

        // TS 写 → Rust 读：seed 行就是冻结的 TS 创建行，Rust 列表必须原样读回（含固定 id/时间戳）。
        const initial = (await service.listAutomations(scope as never)) as AutomationRow[];
        expect(initial).toEqual(expected.afterTsCreate);

        // Rust 写 → 读：原用例由 TS 读回 Rust 生成的行；Phase 3 不再有 TS oracle，
        // 改为比较 Rust 返回行与列表对冻结 TS 投影的归一化结果。created_at 必须晚于 seed 行，
        // 列表按 created_at DESC 的 [新行, seed 行] 顺序才稳定。
        await new Promise((done) => setTimeout(done, 5));
        const fromRust = (await service.createAutomation({
          ...scope,
          title: "weekly report",
          cronExpr: "0 10 * * 1",
          prompt: "write report",
          recurring: true,
        } as never)) as AutomationRow;
        const labels = new Map([[String(fromRust.automationId), RUST_LABEL]]);
        expect(normalize(fromRust, labels)).toEqual(expected.rustCreated);
        const afterRustCreate = (await service.listAutomations(scope as never)) as AutomationRow[];
        // 原用例的 `tsList` 必须原样包含 Rust 创建返回行；这里用 Rust 列表读回同一行，
        // 保留 create 返回值与列表投影（含动态时间戳）逐字段全等的检查。
        expect(afterRustCreate.find((row) => row.automationId === fromRust.automationId)).toEqual(
          fromRust,
        );
        expect(normalize(afterRustCreate, labels)).toEqual(expected.afterRustCreate);

        // 原用例 Rust 暂停 → TS get；现在直接对 Rust 列表做同一投影比较，
        // 并保留原断言只校验的两个字段。
        await service.setAutomationEnabled({
          automationId: fromRust.automationId,
          enabled: false,
          ...scope,
        } as never);
        const afterPause = (await service.listAutomations(scope as never)) as AutomationRow[];
        const paused = afterPause.find((row) => row.automationId === fromRust.automationId);
        expect(paused).toMatchObject(expected.pausedFields);
        expect(normalize(paused, labels)).toEqual(expected.pausedRow);
        expect(normalize(afterPause, labels)).toEqual(expected.afterPause);

        // 原用例由 TS 改名、Rust 读回；没有 TS 写者后改名也走 Rust updateAutomation，
        // 再与冻结的 TS 改名观察比较（保留原用例只断言的新标题）。
        await service.updateAutomation({
          automationId: expected.tsCreated.automationId,
          title: expected.renamedTitle,
          ...scope,
        } as never);
        const afterRename = (await service.listAutomations(scope as never)) as AutomationRow[];
        expect(
          afterRename.find((row) => row.automationId === expected.tsCreated.automationId),
        ).toMatchObject({ title: expected.renamedTitle });
        expect(normalize(afterRename, labels)).toEqual(expected.afterRename);

        // 原用例 Rust 删除 → TS 列表只剩 TS 创建行；现在保留「只剩一行且 id 正确」的断言，
        // 再与冻结的 TS 列表观察比较。
        await service.deleteAutomation({
          automationId: fromRust.automationId,
          ...scope,
        } as never);
        const afterDelete = (await service.listAutomations(scope as never)) as AutomationRow[];
        expect(afterDelete).toEqual([
          expect.objectContaining({ automationId: expected.tsCreated.automationId }),
        ]);
        expect(normalize(afterDelete, labels)).toEqual(expected.afterDelete);
      } finally {
        await host.stop();
        await rm(root, { recursive: true, force: true });
      }
    },
  );
});
