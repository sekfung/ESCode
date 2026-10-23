import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, realpath, symlink, writeFile } from "node:fs/promises";
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
import { appSettingsSchema } from "@escode/shared";

// Phase 3 golden freeze：readdir 与 tasks-index 的 TS 观察已冻结为仓内资产
// （packages/desktop/test/fixtures/golden/）。测试不再实例化 createFileService/TaskIndexRepo，
// 删除 TS 实现后仍能独立重验。
const goldenFile = new URL("./fixtures/golden/rustHostRpc.oracle.json", import.meta.url);
const goldenTasksDb = new URL("./fixtures/golden/rustHostRpc.tasks.sqlite", import.meta.url);

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
        // 冻结的 TS tasks-index 库：Rust Host 直接读同一文件，不再在测试内跑 TS Repo 造数。
        await mkdir(join(dataBaseDir, ".escode", "v2"), { recursive: true });
        await copyFile(goldenTasksDb, join(dataBaseDir, ".escode", "v2", "tasks-index.sqlite"));
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const addr = String(listening.addr);
        const secret = String(listening.secret);

        // 真实客户端：connectViaProtocol（与 desktop 连接远端 Host 同一条路径）。
        const services = await connectRpc(addr, secret);
        const settings = (await services.settingService.get()) as Record<string, unknown>;
        // 缺文件时必须与 TS `appSettingsSchema.parse({})` 的默认对象全等（切唯一 Host 后同一视图）。
        expect(settings).toEqual(appSettingsSchema.parse({}));

        // 写入 → 落盘（Rust settings 域）→ 再读回。
        await services.settingService.update({ locale: "zh-CN" } as never);
        const updated = (await services.settingService.get()) as Record<string, unknown>;
        expect(updated.locale).toBe("zh-CN");
        const onDisk = JSON.parse(
          await readFile(join(home, ".escode", "v2", "setting.json"), "utf8"),
        ) as Record<string, unknown>;
        expect(onDisk.locale).toBe("zh-CN");

        // hooks 域（M3）：空 workspace 没有声明，真实客户端拿到与 TS 同形结果。
        const emptyHooks = (await services.hooksService.loadHooks({
          workspacePath: home,
          workspaceIdentity: home,
        } as never)) as { hooks: unknown[]; hooksEnabled: boolean };
        expect(emptyHooks).toEqual({ hooks: [], hooksEnabled: false });

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

        const golden = JSON.parse(await readFile(goldenFile, "utf8")) as {
          readdir: {
            visible: Array<{ name: string; path: string; type: string; isSymbolicLink: boolean }>;
            hidden: Array<{ name: string; path: string; type: string; isSymbolicLink: boolean }>;
          };
          tasks: {
            metas: Record<string, unknown>;
            lists: {
              timeline: unknown[];
              pinned: Array<{ taskId: string }>;
              archived: unknown[];
              deletedIds: string[];
            };
            taskLists: Array<{ query: unknown; result: unknown }>;
            grouped: unknown;
          };
        };
        const withCorpusPath = (
          entries: Array<{ name: string; path: string; type: string; isSymbolicLink: boolean }>,
        ) => entries.map((entry) => ({ ...entry, path: join(corpus, entry.name) }));

        const rustEntries = await services.fileService.readdir({ path: corpus });
        expect(rustEntries).toEqual(withCorpusPath(golden.readdir.visible));
        expect(rustEntries.some((entry) => entry.name === ".hidden")).toBe(false);
        await expect(
          services.fileService.readdir({ path: corpus, includeHidden: true }),
        ).resolves.toEqual(withCorpusPath(golden.readdir.hidden));
        // 目录组在前（含指向目录的软链），组内 localeCompare；孤立/悬空软链归文件。
        const types = rustEntries.map((entry) => `${entry.name}:${entry.type}`).slice(0, 4);
        expect(types.some((row) => row.endsWith(":directory"))).toBe(true);

        // tasks-index（优先序 5）：Rust Host 读冻结库，逐字段对冻结观察。
        for (const [taskId, expectedMeta] of Object.entries(golden.tasks.metas)) {
          await expect(
            services.escodeTaskService.getTaskMeta({ workspacePath: "/w", taskId }),
          ).resolves.toEqual(expectedMeta);
        }
        // 缺失任务：TS 回 null，Rust 侧为 undefined（RPC 无值语义）。
        await expect(
          services.escodeTaskService.getTaskMeta({ workspacePath: "/w", taskId: "task_absent" }),
        ).resolves.toBeUndefined();
        // 列表分区：冻结的 `listTaskMetas` 过滤结果（provider=glm、pinned/archived 组合）。
        await expect(
          services.escodeTaskService.listTasks({ workspacePath: "/w" } as never),
        ).resolves.toEqual(golden.tasks.lists.timeline);
        await expect(
          services.escodeTaskService.listPinnedTasks({ workspacePath: "/w" } as never),
        ).resolves.toEqual(golden.tasks.lists.pinned);
        await expect(
          services.escodeTaskService.listArchivedTasks({ workspacePath: "/w" } as never),
        ).resolves.toEqual(golden.tasks.lists.archived);
        await expect(services.escodeTaskService.listPinnedTaskIds()).resolves.toEqual(
          golden.tasks.lists.pinned.map((meta) => meta.taskId),
        );
        await expect(
          services.escodeTaskService.listDeletedTaskIds({ workspacePath: "/w" } as never),
        ).resolves.toEqual(golden.tasks.lists.deletedIds);

        // listTaskList（全文搜索 + 分区 + 分页）与分组结构：对冻结的 Repo 观察逐字段比较。
        const scopes = [{ workspacePath: "/w", workspacePurpose: "project" }];
        const queries = [
          { kind: "timeline", sortBy: "updated" },
          { kind: "pinned", sortBy: "updated" },
          { kind: "archived", sortBy: "updated" },
          { kind: "timeline", sortBy: "created", limit: 2 },
          { kind: "timeline", sortBy: "updated", search: "迁移" },
          { kind: "timeline", sortBy: "updated", search: "评审" },
          // 多命中 + 代理对文本：窗口半径/去重/上限与 JS 逐字符对齐。
          { kind: "timeline", sortBy: "updated", search: "迁移" },
          { kind: "timeline", sortBy: "updated", search: "🙂" },
        ] as const;
        for (const [index, query] of queries.entries()) {
          const rustList = await services.escodeTaskService.listTaskList({
            workspaceScopes: scopes,
            ...query,
          } as never);
          expect(rustList, JSON.stringify(query)).toEqual(golden.tasks.taskLists[index].result);
        }
        await expect(
          services.escodeTaskService.listGroupedTaskViewStructure({
            workspaceScopes: scopes,
          } as never),
        ).resolves.toEqual(golden.tasks.grouped);

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

        // hooks 域写读：真实客户端 saveHooks → 落盘 → loadHooks（差分见 rustHostHooksParity.test.ts）。
        await mkdir(join(home, ".escode", "cli"), { recursive: true });
        await writeFile(
          join(home, ".escode", "cli", "config.json"),
          JSON.stringify({ features: { mcp: true } }),
        );
        await services.hooksService.saveHooks({
          workspacePath,
          workspaceIdentity: workspacePath,
          hooks: [
            {
              id: "rpc-hook",
              event: "Stop",
              type: "process",
              command: "node",
              args: ["stop.mjs"],
              enabled: true,
              location: { source: "escode", scope: "project" },
            },
          ],
        } as never);
        const loadedHooks = (await services.hooksService.loadHooks({
          workspacePath,
          workspaceIdentity: workspacePath,
        } as never)) as {
          hooks: Array<Record<string, unknown>>;
          hooksEnabled: boolean;
        };
        expect(loadedHooks.hooksEnabled).toBe(true);
        expect(loadedHooks.hooks[0]?.command).toBe("node");
        expect(
          JSON.parse(await readFile(join(home, ".escode", "cli", "config.json"), "utf8")),
        ).toEqual({ features: { mcp: true }, hooks: { events: {} } });

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
