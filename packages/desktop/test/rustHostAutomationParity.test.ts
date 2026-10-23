// automation 域的真实客户端 + TS 差分（docs/specs/rust-u2-automation.md §6）。
//
// 同一 tasks-index 库由 TS `AutomationService` 与 Rust Host 交替写读：一侧创建/更新/删除后，
// 另一侧经真实 `@escode/client` 或真实 TS 服务读回，逐字段全等。这样验证的是两侧对同一
// SQLite 行的读取投影与写入语义，而不是各写各的夹具。
import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";
// 相对路径 import：根 node_modules 不链接工作区包，测试要拿真实 TS 服务作 oracle。
import { AutomationService } from "../../services/src/session/automationService.js";
import { AutomationRepo } from "../../services/src/session/automationRepo.js";
import { prepareTasksIndexStorage } from "../../services/src/storage-startup.js";

describe("rust host automation parity", () => {
  it.skipIf(!existsSync(hostBinary))(
    "shares automations with the TypeScript service in both directions",
    { timeout: 60_000 },
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-automation-parity-"));
      const home = join(root, "home");
      const dataBaseDir = join(root, "data-base");
      const workspacePath = join(root, "workspace");
      for (const path of [home, dataBaseDir, workspacePath]) await mkdir(path, { recursive: true });

      const dbPath = join(dataBaseDir, ".escode", "v2", "tasks-index.sqlite");
      await prepareTasksIndexStorage(dbPath, () => {});
      const ts = new AutomationService(new AutomationRepo(dbPath));
      const scope = { workspacePath, workspaceIdentity: workspacePath };

      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));

        // TS 写 → Rust 读：同一行、同一投影。
        const fromTs = await ts.create({
          ...scope,
          title: "daily check",
          cronExpr: "0 9 * * *",
          prompt: "check logs",
          recurring: true,
        });
        const rustList = (await services.escodeAgentService.listAutomations(
          scope as never,
        )) as unknown[];
        expect(rustList).toEqual([fromTs]);

        // Rust 写 → TS 读：Rust 生成的行 TS 必须能原样读回。
        // 列表按 created_at DESC；等到下一毫秒再建，避免同毫秒并列导致顺序不稳定。
        await new Promise((done) => setTimeout(done, 5));
        const fromRust = (await services.escodeAgentService.createAutomation({
          ...scope,
          title: "weekly report",
          cronExpr: "0 10 * * 1",
          prompt: "write report",
          recurring: true,
        } as never)) as Record<string, unknown>;
        const tsList = await ts.list(scope);
        expect(tsList).toEqual([fromRust, fromTs]);

        // 两侧交替更新：Rust 暂停 → TS 看到 paused；TS 改名 → Rust 看到新标题。
        await services.escodeAgentService.setAutomationEnabled({
          automationId: fromRust.automationId,
          enabled: false,
          ...scope,
        } as never);
        const pausedByTs = await ts.get(String(fromRust.automationId), scope);
        expect(pausedByTs).toMatchObject({ enabled: false, lifecycleStatus: "paused" });

        await ts.update(String(fromTs.automationId), { title: "daily check (renamed)" }, scope);
        const renamedByRust = (await services.escodeAgentService.listAutomations(
          scope as never,
        )) as Array<Record<string, unknown>>;
        expect(renamedByRust.find((row) => row.automationId === fromTs.automationId)).toMatchObject(
          {
            title: "daily check (renamed)",
          },
        );

        // Rust 删除 → TS 列表只剩 TS 创建的那条。
        await services.escodeAgentService.deleteAutomation({
          automationId: fromRust.automationId,
          ...scope,
        } as never);
        expect(await ts.list(scope)).toEqual([
          expect.objectContaining({ automationId: fromTs.automationId }),
        ]);
      } finally {
        await host.stop();
      }
    },
  );
});
