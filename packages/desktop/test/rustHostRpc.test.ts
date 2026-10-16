import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, realpath, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SocketProtocol, ChannelClient } from "@escode/rpc";
import {
  connectRpc,
  hostBinary,
  nextMeaningful,
  openHostSocket,
  startHost,
  wrapNodeWebSocket,
} from "./rustHostTestSupport.js";
import { createFileService } from "@escode/services/node";
import { prepareTasksIndexStorage } from "@escode/services/storage-startup";
// 相对路径 import：根 node_modules 不链接工作区包，测试要拿真实 Repo 作 oracle。
import { TaskIndexRepo } from "../../services/src/session/taskIndexRepo.js";

// docs/specs/rust-u2-host-skeleton.md §3.4 的「TS 接线」证明：**真实 `@escode/client`**
// 经 `@escode/rpc` 协议连 Rust Host 的 RPC 面（WebSocket），调用 `settingService` 成功，
// 并对未迁移的 channel/method 收到显式 NotImplemented。
//
// 链路：spawn escode-host → 控制消息 rpcListen{bind,home} → 得到 addr + 进程密钥 → 申请 capability → ws://addr/ws/host →
//       SocketProtocol(WS) → connectViaProtocol → services.settingService。

describe("rust host rpc surface", () => {
  it.skipIf(!existsSync(hostBinary))(
    "serves the real client with settings and explicit gaps",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-rpc-"));
      const home = join(root, "home");
      const host = startHost(home);
      try {
        const ready = await nextMeaningful(host);
        expect(ready.kind).toBe("hostReady");

        // Main 让 Host 开 RPC 面（设置根目录与应用数据根由 Main 指定；dataBaseDir 故意与 home 不同，
        // 用来证明会话工作区取 Main 提交的值，而不是 Host 进程 HOME 派生）。
        const dataBaseDir = join(root, "data-base");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const addr = String(listening.addr);
        const secret = String(listening.secret);

        // 真实客户端：connectViaProtocol（与 desktop 连接远端 Host 同一条路径）。
        const services = await connectRpc(addr, secret);
        const settings = (await services.settingService.get()) as Record<string, unknown>;
        expect(settings).toBeTypeOf("object");

        // 写入 → 落盘（Rust settings 域）→ 再读回。
        await services.settingService.update({ locale: "zh-CN" } as never);
        const updated = (await services.settingService.get()) as Record<string, unknown>;
        expect(updated.locale).toBe("zh-CN");
        const onDisk = JSON.parse(
          await readFile(join(home, ".escode", "v2", "setting.json"), "utf8"),
        ) as Record<string, unknown>;
        expect(onDisk.locale).toBe("zh-CN");

        // 未迁移的 method：显式 NotImplemented（不挂起）。
        await expect(services.serialService.list()).rejects.toThrow(
          /does not implement channel `serial`/,
        );

        // escode-session 域：Host 转成 runtime 的 session/list（TS Host 同构），
        // 新 workspace 返回空列表；实例按 workspace 建立。
        const workspacePath = join(root, "workspace");
        await mkdir(workspacePath, { recursive: true });
        const sessions = (await services.escodeSessionService.listSessions({
          workspacePath,
          workspaceIdentity: workspacePath,
        } as never)) as unknown[];
        expect(Array.isArray(sessions)).toBe(true);
        expect(sessions).toHaveLength(0);

        // 读面转发：readSession 对不存在的会话透传运行时语义（RuntimeError）。
        await expect(
          services.escodeSessionService.readSession({
            workspacePath,
            workspaceIdentity: workspacePath,
            sessionId: "sess_missing",
            deliveryKind: "desktop-continuous",
          } as never),
        ).rejects.toThrow(/Session unavailable/);

        // file 域精确子集：stat / checkFilesExist / resolvePath / readFileRange。
        const probeFile = join(workspacePath, "probe.txt");
        await writeFile(probeFile, "hello world");
        const fileStat = (await services.fileService.stat({ path: probeFile })) as Record<
          string,
          unknown
        >;
        expect(fileStat.type).toBe("file");
        expect(fileStat.size).toBe(11);
        expect(typeof fileStat.mtimeMs).toBe("number");
        const dirStat = (await services.fileService.stat({ path: workspacePath })) as Record<
          string,
          unknown
        >;
        expect(dirStat.type).toBe("directory");
        expect(dirStat.size).toBeUndefined();

        const existence = (await services.fileService.checkFilesExist({
          paths: [probeFile, workspacePath, join(workspacePath, "missing.txt")],
        })) as Array<{ path: string; exists: boolean }>;
        expect(existence.map((row) => row.exists)).toEqual([true, false, false]);

        const resolved = await services.fileService.resolvePath({ path: workspacePath });
        expect(await realpath(workspacePath)).toBe(resolved);

        const range = (await services.fileService.readFileRange({
          path: probeFile,
          offset: 6,
          length: 5,
        })) as Uint8Array;
        expect(new TextDecoder().decode(range)).toBe("world");

        const textSlice = (await services.fileService.readTextFile({
          path: probeFile,
          offset: 6,
          length: 5,
        })) as Record<string, unknown>;
        expect(textSlice.content).toBe("world");
        expect(textSlice.bytesRead).toBe(5);
        expect(textSlice.totalBytes).toBe(11);
        expect(textSlice.truncated).toBe(false);
        expect(textSlice.isBinary).toBe(false);

        const binaryFile = join(workspacePath, "probe.bin");
        await writeFile(binaryFile, Buffer.from([1, 0, 2, 3, 4, 5]));
        const binarySlice = (await services.fileService.readTextFile({
          path: binaryFile,
        })) as Record<string, unknown>;
        expect(binarySlice.isBinary).toBe(true);
        expect(binarySlice.content).toBe("");

        const mediaPreview = (await services.fileService.readMediaPreview({
          path: probeFile,
        })) as Record<string, unknown>;
        expect(mediaPreview.mediaType).toBe("application/octet-stream");
        expect(Buffer.from(String(mediaPreview.dataBase64), "base64").toString("utf8")).toBe(
          "hello world",
        );

        const binaryPreview = (await services.fileService.readBinaryPreview({
          path: binaryFile,
        })) as Record<string, unknown>;
        expect(Buffer.from(String(binaryPreview.dataBase64), "base64")).toEqual(
          Buffer.from([1, 0, 2, 3, 4, 5]),
        );

        // 工作区创建：默认工作区（home/ESCodeProject）与命名工作区（含名称校验）。
        const defaultWorkspace = (await services.fileService.createDefaultWorkspace()) as {
          path: string;
        };
        expect(defaultWorkspace.path).toBe(join(home, "ESCodeProject"));
        const scratch = (await services.fileService.createScratchWorkspace({
          name: "demo-case",
        })) as { path: string };
        expect(scratch.path).toBe(join(home, "ESCodeProject", "demo-case"));
        await expect(services.fileService.createScratchWorkspace({ name: "a/b" })).rejects.toThrow(
          /cannot contain path separators/,
        );

        // readdir：与真实 TS fileService 逐字段差分（排序是 `localeCompare`——ICU collation，
        // 不是码点序；类型/隐藏项/软链判定同 TS）。两侧同 locale（测试进程 env 决定）。
        const corpus = join(root, "corpus");
        await mkdir(corpus, { recursive: true });
        const corpusNames = [
          "_x.txt",
          "a.txt",
          "A.txt",
          "dir_b",
          "dirA",
          "é.txt",
          "README.md",
          "z.txt",
          "Z",
          "1.txt",
          "10.txt",
          "2.txt",
          "a 1",
          "a_1",
          "a-1",
          "a.1",
          "file.TXT",
          "FILE.txt",
          "file(1).txt",
          ".hidden",
        ];
        for (const name of corpusNames) {
          await writeFile(join(corpus, name), "x");
        }
        await mkdir(join(corpus, "Zenith"), { recursive: true });
        await mkdir(join(corpus, "áz"), { recursive: true });
        await symlink(join(corpus, "dirA"), join(corpus, "link-to-dir"));
        await symlink(join(corpus, "missing-target"), join(corpus, "dangling-link"));

        const rustEntries = await services.fileService.readdir({ path: corpus });
        const tsEntries = await createFileService().readdir({ path: corpus });
        expect(rustEntries).toEqual(tsEntries);
        expect(rustEntries.some((entry) => entry.name === ".hidden")).toBe(false);
        await expect(
          services.fileService.readdir({ path: corpus, includeHidden: true }),
        ).resolves.toEqual(
          await createFileService().readdir({ path: corpus, includeHidden: true }),
        );
        // 目录组在前（含指向目录的软链），组内 localeCompare；孤立/悬空软链归文件。
        const types = rustEntries.map((entry) => `${entry.name}:${entry.type}`).slice(0, 4);
        expect(types.some((row) => row.endsWith(":directory"))).toBe(true);

        // tasks-index（优先序 5）：TS 用真实 Repo 写、Rust Host 用真实客户端读，逐字段比对。
        // 库路径 `<dataBaseDir>/.escode/v2/tasks-index.sqlite`（TS `getTasksIndexDatabasePath`）。
        await prepareTasksIndexStorage(
          join(dataBaseDir, ".escode", "v2", "tasks-index.sqlite"),
          () => {},
        );
        const repo = new TaskIndexRepo(join(dataBaseDir, ".escode", "v2", "tasks-index.sqlite"));
        await repo.ensureReady();
        const seeded = [
          {
            taskId: "task_min",
            traceId: "escode-task_min",
            title: "最小任务",
            workspacePath: "/w",
            createdAt: 100,
            updatedAt: 200,
            mode: "build" as const,
          },
          {
            taskId: "task_full",
            traceId: "escode-task_full",
            title: "完整任务",
            workspacePath: "/w",
            workspaceIdentity: "/w",
            createdAt: 300,
            updatedAt: 400,
            mode: "plan" as const,
            model: "glm-4",
            provider: "glm" as const,
            cronAutomationId: "cron_1",
            offPeakTaskId: "off_peak_1",
            unreadAt: 450,
            status: "running" as const,
          },
        ];
        for (const meta of seeded) {
          await repo.seedTaskMetaIfMissing(meta as never);
        }
        for (const meta of seeded) {
          const rustMeta = await services.escodeTaskService.getTaskMeta({
            workspacePath: "/w",
            taskId: meta.taskId,
          });
          const tsMeta = await repo.getTaskMeta({ workspacePath: "/w", taskId: meta.taskId });
          expect(rustMeta).toEqual(tsMeta);
        }
        // 缺失任务：TS 回 null，Rust 侧为 undefined（RPC 无值语义）。
        await expect(
          services.escodeTaskService.getTaskMeta({ workspacePath: "/w", taskId: "task_absent" }),
        ).resolves.toBeUndefined();
        expect(await repo.getTaskMeta({ workspacePath: "/w", taskId: "task_absent" })).toBeNull();
        // 列表分区：与真实 Repo 的 `listTaskMetas` 同一过滤组合逐字段比对（provider=glm、
        // pinned/archived 组合；悬空列 provider 为空的行被 glm 过滤排除，与 TS 同）。
        const glm = "glm" as never;
        const listFixtures = [
          { taskId: "task_l1", provider: glm, title: "列表一", pinned: false, archived: false },
          { taskId: "task_l2", provider: glm, title: "列表二", pinned: true, archived: false },
          { taskId: "task_l3", provider: glm, title: "归档", pinned: false, archived: true },
          { taskId: "task_l4", provider: glm, title: "置顶归档", pinned: true, archived: true },
        ];
        for (const fixture of listFixtures) {
          await repo.seedTaskMetaIfMissing({
            taskId: fixture.taskId,
            traceId: `escode-${fixture.taskId}`,
            title: fixture.title,
            workspacePath: "/w",
            createdAt: 10,
            updatedAt: 20,
            mode: "build",
            provider: glm,
          } as never);
          await repo.syncTaskMeta({
            meta: {
              taskId: fixture.taskId,
              traceId: `escode-${fixture.taskId}`,
              title: fixture.title,
              workspacePath: "/w",
              createdAt: 10,
              updatedAt: 20,
              mode: "build",
              provider: glm,
            },
            pinned: fixture.pinned,
            archived: fixture.archived,
          } as never);
        }
        const tsList = (extra: Record<string, unknown>) =>
          repo.listTaskMetas({ workspacePath: "/w", provider: glm, ...extra } as never);
        expect(
          await services.escodeTaskService.listTasks({ workspacePath: "/w" } as never),
        ).toEqual(await tsList({ pinned: false, archived: false }));
        expect(
          await services.escodeTaskService.listPinnedTasks({ workspacePath: "/w" } as never),
        ).toEqual(await tsList({ pinned: true, archived: false }));
        expect(
          await services.escodeTaskService.listArchivedTasks({ workspacePath: "/w" } as never),
        ).toEqual(await tsList({ archived: true }));
        expect(await services.escodeTaskService.listPinnedTaskIds()).toEqual(
          (await tsList({ pinned: true, archived: false })).map((meta) => meta.taskId),
        );
        expect(
          await services.escodeTaskService.listDeletedTaskIds({ workspacePath: "/w" } as never),
        ).toEqual(await repo.listDeletedTaskIds({ workspacePath: "/w", provider: glm } as never));

        // listTaskList（全文搜索 + 分区 + 分页）与分组结构：与真实 Repo 逐字段比对。
        // 搜索文本列由 SQL 直接写（TS 读路径同源，写路径属 S2）。
        const sqlitePath = join(dataBaseDir, ".escode", "v2", "tasks-index.sqlite");
        const raw = new (await import("node:sqlite")).DatabaseSync(sqlitePath);
        for (const [taskId, provider, pinned, archived, updatedAt, searchable] of [
          ["task_q1", "glm", 0, 0, 500, "部署脚本与回滚说明"],
          ["task_q2", "glm", 0, 0, 400, "评审 Rust Host 的迁移"],
          ["task_q3", "glm", 1, 0, 300, "置顶：迁移检查清单"],
          ["task_q4", "glm", 0, 1, 200, "已归档内容"],
          ["task_q5", "claude", 0, 0, 600, "历史导入不应出现"],
          // 星平面字符（UTF-16 代理对）与多命中：摘要窗口必须按码元切，与 JS slice 同。
          [
            "task_q6",
            "glm",
            0,
            0,
            700,
            "✅ 部署完成后核对迁移清单：迁移包含回滚与灰度，迁移窗口 30 分钟 🙂 结束",
          ],
        ] as const) {
          raw
            .prepare(
              `INSERT INTO tasks (workspace_key, workspace_path, task_id, title, mode, provider,
               created_at, updated_at, pinned, archived, deleted, searchable_text, meta_json)
             VALUES ('/w', '/w', ?, ?, 'build', ?, 10, ?, ?, ?, 0, ?, ?)`,
            )
            .run(
              taskId,
              `标题 ${taskId}`,
              provider,
              updatedAt,
              pinned,
              archived,
              searchable,
              JSON.stringify({
                taskId,
                traceId: `escode-${taskId}`,
                title: `标题 ${taskId}`,
                workspacePath: "/w",
                createdAt: 10,
                updatedAt,
                mode: "build",
              }),
            );
        }
        raw
          .prepare(
            `INSERT INTO task_groups (group_id, title, color, created_at, updated_at)
           VALUES ('group_diff', '差分组', 'blue', 10, 20)`,
          )
          .run();
        raw
          .prepare(
            `INSERT INTO task_group_members
             (group_id, workspace_key, workspace_path, workspace_identity, task_id, sort_order,
              added_at, created_at, updated_at)
           VALUES ('group_diff', '/w', '/w', NULL, 'task_q1', 7, 30, 30, 30)`,
          )
          .run();
        raw
          .prepare(
            `INSERT INTO task_group_view_node_orders (node_type, node_key, sort_order, created_at, updated_at)
           VALUES ('group', 'group_diff', 1, 1, 1), ('task', '["/w","task_q2"]', 2, 1, 1)`,
          )
          .run();
        raw.close();

        const scopes = [{ workspacePath: "/w", workspacePurpose: "project" }];
        for (const query of [
          { kind: "timeline", sortBy: "updated" },
          { kind: "pinned", sortBy: "updated" },
          { kind: "archived", sortBy: "updated" },
          { kind: "timeline", sortBy: "created", limit: 2 },
          { kind: "timeline", sortBy: "updated", search: "迁移" },
          { kind: "timeline", sortBy: "updated", search: "评审" },
          // 多命中 + 代理对文本：窗口半径/去重/上限与 JS 逐字符对齐。
          { kind: "timeline", sortBy: "updated", search: "迁移" },
          { kind: "timeline", sortBy: "updated", search: "🙂" },
        ] as const) {
          const rustList = await services.escodeTaskService.listTaskList({
            workspaceScopes: scopes,
            ...query,
          } as never);
          const tsList = await repo.queryTaskList({
            workspaceScopes: scopes,
            ...query,
            provider: "glm",
          } as never);
          expect(rustList, JSON.stringify(query)).toEqual(tsList);
        }
        expect(
          await services.escodeTaskService.listGroupedTaskViewStructure({
            workspaceScopes: scopes,
          } as never),
        ).toEqual(await repo.queryGroupedTaskViewStructure({ workspaceScopes: scopes } as never));

        // 未迁移方法：显式缺口（列出已实现的方法）。
        await expect(
          services.escodeTaskService.createTask({ workspacePath: "/w" } as never),
        ).rejects.toThrow(/does not implement `escode-task\.createTask` yet/);
        repo.close();

        // 会话工作区：`<dataBaseDir>/.escode/workspace/default`（TS `getConversationWorkspaceDir()`），
        // 首建 created=true，再调 created=false。
        const conversation = await services.fileService.ensureConversationWorkspace();
        const expectedConversation = join(dataBaseDir, ".escode", "workspace", "default");
        expect(conversation.path).toBe(expectedConversation);
        expect(conversation.created).toBe(true);
        expect(conversation.workspacePurpose).toBe("conversation");
        expect(existsSync(expectedConversation)).toBe(true);
        const conversationAgain = await services.fileService.ensureConversationWorkspace();
        expect(conversationAgain.created).toBe(false);

        // file 域已 17/17（搜索/索引见 rustHostFileIndex.test.ts）；用仍未注册的 channel 验证显式缺口。
        await expect(services.serialService.getSnapshot({} as never)).rejects.toThrow(
          /does not implement channel `serial`/,
        );

        // system 域：info() 返回 homedir + Node 口径 platform。
        const systemInfo = (await services.systemService.info()) as Record<string, unknown>;
        expect(systemInfo.homedir).toBe(home);
        expect(["darwin", "win32", "linux", "freebsd", "openbsd", "sunos", "aix"]).toContain(
          systemInfo.platform,
        );

        // workspace 预热：等运行时报到后返回协议常量（首屏不因 agent 缺失崩溃）。
        const initialized = (await services.escodeSessionService.initializeWorkspace({
          workspacePath,
          workspaceIdentity: workspacePath,
        } as never)) as Record<string, unknown>;
        expect(initialized.available).toBe(true);
        expect(initialized.protocolName).toBe("ESCode Protocol");
        expect(initialized.protocolVersion).toBe(1);
        expect(initialized.transportKind).toBe("stdio");

        // workspace 展示面：先读运行时能力再带执行能力位读展示面（TS agent 层同序）。
        const presentation = (await services.escodeSessionService.readWorkspacePresentation({
          workspacePath,
          workspaceIdentity: workspacePath,
        } as never)) as Record<string, unknown>;
        expect(presentation.mode).toBe("build");
        expect(Array.isArray(presentation.slashCommands)).toBe(true);

        // 写面转发：TS `closeSession` 回 undefined（`.then(() => undefined)`）；条件关闭结果
        // 属 `closeDeferredDraftSession`（未加载会话为 false）。
        const closed = await services.escodeSessionService.closeSession({
          workspacePath,
          workspaceIdentity: workspacePath,
          sessionId: "sess_missing",
        } as never);
        expect(closed).toBeUndefined();
        const draftClosed = await services.escodeSessionService.closeDeferredDraftSession({
          workspacePath,
          workspaceIdentity: workspacePath,
          sessionId: "sess_missing",
        } as never);
        expect(draftClosed).toBe(false);

        // createSession 已迁移：无模型配置时由运行时拒绝（语义错误透传，不是缺口回显）。
        await expect(
          services.escodeSessionService.createSession({
            workspacePath,
            workspaceIdentity: workspacePath,
          } as never),
        ).rejects.toThrow(/^(?!.*does not implement)/);

        // 未迁移的 channel：显式 NotImplemented，且消息里带已实现清单。
        const client = new ChannelClient(
          new SocketProtocol(wrapNodeWebSocket(await openHostSocket(addr, secret))),
        );
        await expect(client.getChannel("workspace").call("query", {})).rejects.toThrow(
          /does not implement channel `workspace` yet .*setting/,
        );
      } finally {
        await host.stop();
      }
    },
  );
});
