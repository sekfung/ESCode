// 远端 RPC server 的最小 TS fixture：用真实 `ChannelServer` / `SocketProtocol` 说话，
// 供 Rust `RemoteChannelClient` 做跨语言帧面/序列化/握手对拍。
// 运行：node --import tsx <this file>
import { VSBuffer } from "../../../../packages/rpc/src/buffer.ts";
import { ChannelServer } from "../../../../packages/rpc/src/channelServer.ts";
import { Emitter } from "../../../../packages/rpc/src/foundation.ts";
import { SocketProtocol } from "../../../../packages/rpc/src/protocol.ts";
import { registerProvisioningChannel } from "./rpc_server_fixture_provisioning.mjs";
import { registerRuntimePreferencesChannel } from "./rpc_server_fixture_runtime_preferences.mjs";

const socket = {
  onData: (listener) => {
    const handler = (chunk) => listener(VSBuffer.wrap(new Uint8Array(chunk)));
    process.stdin.on("data", handler);
    return { dispose: () => process.stdin.off("data", handler) };
  },
  onClose: () => ({ dispose: () => undefined }),
  onEnd: () => ({ dispose: () => undefined }),
  write: (buffer) => {
    process.stdout.write(
      Buffer.from(buffer.buffer, buffer.byteOffset, buffer.byteLength),
    );
  },
  end: () => process.exit(0),
  drain: async () => undefined,
  dispose: () => undefined,
};

const channel = {
  call: async (_ctx, command, arg) => {
    if (command === "getSnapshot" || command === "getCapabilities") {
      return { ok: true, echo: arg ?? null };
    }
    if (command === "big") {
      return "x".repeat(100_000);
    }
    if (command === "fail") {
      const error = new Error("denied");
      error.name = "SourceServerError";
      error.code = "E_AUTH";
      throw error;
    }
    throw new Error(`unknown command ${command}`);
  },
  listen: (_ctx, event) => {
    const emitter = new Emitter();
    const timer = setInterval(() => emitter.fire({ event }), 10);
    return (listener) => {
      const subscription = emitter.event(listener);
      return {
        dispose: () => {
          subscription.dispose();
          clearInterval(timer);
        },
      };
    };
  },
};

/**
 * 与 `packages/server/src/entry-stdio.ts` 的 `waitForAck` 对等：hello 之后必须先消费
 * `escode-hello-ack` 行，之后 stdin 才是 RPC 帧。ack 之后的残余字节回推给 RPC。
 */
async function readHelloAckLine() {
  return new Promise((resolve) => {
    let buffer = "";
    const onData = (chunk) => {
      buffer += chunk.toString("utf8");
      const newlineIndex = buffer.indexOf("\n");
      if (newlineIndex === -1) return;
      process.stdin.off("data", onData);
      const remaining = buffer.slice(newlineIndex + 1);
      if (remaining.length > 0) {
        process.stdin.unshift(Buffer.from(remaining, "utf8"));
      }
      resolve(buffer.slice(0, newlineIndex).trim());
    };
    process.stdin.on("data", onData);
  });
}

if (process.env.ESCODE_FIXTURE_HELLO === "1") {
  const compatibility = process.env.ESCODE_FIXTURE_COMPATIBILITY
    ? JSON.parse(process.env.ESCODE_FIXTURE_COMPATIBILITY)
    : undefined;
  process.stdout.write(
    `${JSON.stringify({
      type: "escode-hello",
      version: process.env.ESCODE_FIXTURE_VERSION ?? "3.15.2",
      platform: "linux",
      arch: "x64",
      pid: process.pid,
      ...(compatibility ? { compatibility } : {}),
    })}\n`,
  );
  const ack = JSON.parse(await readHelloAckLine());
  if (ack.type !== "escode-hello-ack") {
    throw new Error(`invalid hello-ack: ${JSON.stringify(ack)}`);
  }
}

// 自动恢复测试：握手完成后按环境变量退出，模拟 online 后链路断开。
const exitAfterMs = Number.parseInt(
  process.env.ESCODE_FIXTURE_EXIT_AFTER_MS ?? "0",
  10,
);
if (exitAfterMs > 0) {
  setTimeout(() => process.exit(0), exitAfterMs);
}

const server = new ChannelServer(
  new SocketProtocol(socket),
  "fixture",
  1000,
  true,
);
server.registerChannel("test", channel);
// 远端 scope 转发只建立 Host inventory（protos/TS 服务面清单）里存在的 channel 名，
// 因此把同一处理函数再注册到 `window-project-changes`（成员：getCapabilities / onDidChange），
// 供 scoped capability 的跨语言验收使用。
server.registerChannel("window-project-changes", {
  call: (ctx, command, arg) => channel.call(ctx, command, arg),
  listen: (ctx, event) => channel.listen(ctx, event),
});

// `source-server-workspace`（S4-f）：迁移计数/报告的最小 owner。延迟与失败由环境变量控制，
// 供 Host 验证 loading → available、unavailable 与迟到结果丢弃。
const migrationCountDelayMs = Number.parseInt(
  process.env.ESCODE_FIXTURE_MIGRATION_COUNTS_DELAY_MS ?? "0",
  10,
);
const migrationCountFailure =
  process.env.ESCODE_FIXTURE_MIGRATION_COUNTS_FAIL === "1";
const migrationReport = () => ({
  rows: [
    {
      migrationId: "migration-1",
      workspacePath: "/remote/workspace",
      canonicalKey: "remote:server:fixture:/remote/workspace",
      variantKeys: [],
      status: "completed",
      updatedAt: 100,
      conflicts: [],
    },
  ],
});
const sourceServerWorkspaceChannel = {
  call: async (_ctx, command, rawArg) => {
    if (migrationCountDelayMs > 0) {
      await new Promise((resolve) =>
        setTimeout(resolve, migrationCountDelayMs),
      );
    }
    switch (command) {
      case "getFailedPathCount":
        if (migrationCountFailure)
          throw new Error("failed-path-count unavailable");
        return 2;
      case "getAbandonedPathCount":
        if (migrationCountFailure)
          throw new Error("abandoned-path-count unavailable");
        return 3;
      case "listMigrationReports":
      case "abandonFailedWorkspace":
      case "retryWorkspaceMigration":
        return migrationReport();
      case "resolveWorkspace": {
        const params = objectArg(rawArg);
        return { canonicalWorkspaceKey: params.workspacePath, mergedKeys: [] };
      }
      default:
        throw new Error(`unknown command ${command}`);
    }
  },
  listen: () => () => ({ dispose: () => undefined }),
};
server.registerChannel("source-server-workspace", sourceServerWorkspaceChannel);

registerProvisioningChannel(server);
registerRuntimePreferencesChannel(server);

// 远端 window-controller（spec §10.13）的最小 `escode-task` 事实源：内存任务表 +
// `onDynamicWorkspaceEvent` 广播。写面成功后通知 Host 刷新投影（与真实服务一致）。
const taskEvents = new Emitter();
const remoteTasks = [
  {
    taskId: "task-remote-1",
    title: "Remote Alpha",
    workspacePath: "/remote/workspace",
    createdAt: 100,
    updatedAt: 200,
    pinned: false,
    archived: false,
    unread: true,
  },
  {
    taskId: "task-remote-2",
    title: "Remote Beta",
    workspacePath: "/remote/workspace",
    createdAt: 110,
    updatedAt: 300,
    pinned: false,
    archived: false,
    unread: true,
  },
];

function objectArg(arg) {
  return (Array.isArray(arg) ? arg[0] : arg) ?? {};
}

function taskMeta(task, params) {
  return {
    taskId: task.taskId,
    title: task.title,
    workspacePath: params.workspacePath ?? task.workspacePath,
    ...(params.workspaceIdentity
      ? { workspaceIdentity: params.workspaceIdentity }
      : {}),
    createdAt: task.createdAt,
    updatedAt: task.updatedAt,
    mode: "agent",
    status: process.env.ESCODE_FIXTURE_TASK_STATUS ?? "completed",
    unread: task.unread,
  };
}

function tasksInWorkspace(params) {
  const scopes = Array.isArray(params.workspaceScopes)
    ? params.workspaceScopes
    : [];
  if (scopes.length > 0) {
    const paths = new Set(
      scopes
        .map((scope) => scope?.workspacePath)
        .filter((value) => typeof value === "string" && value.length > 0),
    );
    return remoteTasks.filter((task) => paths.has(task.workspacePath));
  }
  return remoteTasks.filter(
    (task) => task.workspacePath === params.workspacePath,
  );
}

function notifyWorkspace(params) {
  taskEvents.fire({
    workspacePath: params.workspacePath ?? task.workspacePath,
    ...(params.workspaceIdentity
      ? { workspaceIdentity: params.workspaceIdentity }
      : {}),
  });
}

const taskChannel = {
  call: async (_ctx, command, rawArg) => {
    const params = objectArg(rawArg);
    const tasks = tasksInWorkspace(params);
    switch (command) {
      case "listTasks":
        return tasks
          .filter((task) => !task.archived && !task.pinned)
          .map((task) => taskMeta(task, params));
      case "listPinnedTasks":
        return tasks
          .filter((task) => !task.archived && task.pinned)
          .map((task) => taskMeta(task, params));
      case "listArchivedTasks":
        return tasks
          .filter((task) => task.archived)
          .map((task) => taskMeta(task, params));
      case "listTaskList": {
        const search =
          typeof params.search === "string"
            ? params.search.trim().toLowerCase()
            : "";
        const items = tasks
          .filter((task) => !task.archived)
          .filter(
            (task) => !search || task.title.toLowerCase().includes(search),
          )
          .map((task) => taskMeta(task, params));
        return { items, total: items.length, hasMore: false };
      }
      case "setTaskPinned":
      case "archiveTask":
      case "unarchiveTask":
      case "setTaskUnread":
      case "deleteTask": {
        const task = tasks.find(
          (candidate) => candidate.taskId === params.taskId,
        );
        if (!task) throw new Error(`task not found: ${params.taskId}`);
        if (command === "setTaskPinned") task.pinned = params.pinned === true;
        if (command === "archiveTask") task.archived = true;
        if (command === "unarchiveTask") task.archived = false;
        if (command === "setTaskUnread") task.unread = params.unread === true;
        task.updatedAt += 1;
        const meta = taskMeta(task, params);
        if (command === "deleteTask")
          remoteTasks.splice(remoteTasks.indexOf(task), 1);
        notifyWorkspace(params);
        return meta;
      }
      case "deleteArchivedTask": {
        const task = tasks.find(
          (candidate) =>
            candidate.taskId === params.taskId && candidate.archived,
        );
        if (!task) return false;
        remoteTasks.splice(remoteTasks.indexOf(task), 1);
        notifyWorkspace(params);
        return true;
      }
      case "deleteArchivedTasks": {
        const deletedTaskIds = [];
        const skippedTaskIds = [];
        for (const taskId of params.taskIds ?? []) {
          const task = remoteTasks.find(
            (candidate) => candidate.taskId === taskId && candidate.archived,
          );
          if (!task) {
            skippedTaskIds.push(taskId);
            continue;
          }
          remoteTasks.splice(remoteTasks.indexOf(task), 1);
          deletedTaskIds.push(taskId);
        }
        notifyWorkspace(params);
        return { deletedTaskIds, skippedTaskIds, failedTaskIds: [] };
      }
      default:
        throw new Error(`unknown command ${command}`);
    }
  },
  listen: (_ctx, event) =>
    event === "onDynamicWorkspaceEvent"
      ? (listener) => taskEvents.event(listener)
      : () => ({ dispose: () => undefined }),
};
if (process.env.ESCODE_FIXTURE_WITHOUT_TASK !== "1") {
  if (process.env.ESCODE_FIXTURE_NO_TASK_CHANNEL !== "1") {
    server.registerChannel("escode-task", taskChannel);
  }
}

// V4 投递模式 fixture（spec §10.15）：只有显式设置模式时才注册，避免影响既有
// remote scope 的 channel 清单断言。Host 必须把 Source Server 的 hello/命令/帧原样转发，
// 不能把 replayable 恢复逻辑拼进 continuous 链路，也不能反过来把模式写死。
const fixtureV4Mode = process.env.ESCODE_FIXTURE_V4_MODE;
if (fixtureV4Mode) {
  const continuous = fixtureV4Mode === "desktop-continuous";
  const agentEvents = new Emitter();
  const agentChannel = {
    call: async (_ctx, command, rawArg) => {
      const params = objectArg(rawArg);
      switch (command) {
        case "helloConversationV4":
          return {
            kind: "hello",
            protocolVersion: 3,
            connectionId: "fixture-connection",
            clientMode: fixtureV4Mode,
            deliveryProfile: continuous ? "continuous" : "replayable",
            serverTime: 1,
            capabilities: {
              nativeDialogs: continuous,
              localTerminal: continuous,
              binaryFrames: false,
              compression: "none",
              workspaceHookReview: true,
              independentPlanState: true,
              workflowRunDeltas: true,
            },
            auth: {},
          };
        case "initializeConversationV4":
          return null;
        case "subscribeConversationV4": {
          const subscriptionId = "fixture-sub";
          const topic = `conversation/${params.sessionId ?? "fixture-session"}`;
          const fire = (frame) =>
            setImmediate(() =>
              agentEvents.fire({ ...frame, topic, subscriptionId }),
            );
          fire({
            kind: "snapshot",
            payload: { rows: [], base: { logEpoch: "fixture", seq: 1 } },
          });
          fire({ kind: "deltas", payload: { seq: 2, deltas: [] } });
          return {
            ack: { subscriptionId, mode: "snapshot", logEpoch: "fixture" },
          };
        }
        case "sendConversationCommandV4": {
          const envelope = params.envelope ?? {};
          const payload = envelope.payload ?? {};
          return {
            status: "accepted",
            commandId: envelope.commandId,
            clientId: envelope.clientId,
            // 与 TS `sendConversationCommandV4` 对齐：模式决定 inputOrigin，Host 不得改写。
            inputOrigin:
              envelope.type === "sendText" && envelope.sessionId
                ? continuous
                  ? "desktop"
                  : "mobile"
                : null,
            text: payload.firstInput?.text ?? null,
          };
        }
        default:
          throw new Error(`unknown command ${command}`);
      }
    },
    listen: (_ctx, event) =>
      event === "onDynamicConversationFrame"
        ? (listener) => agentEvents.event(listener)
        : () => ({ dispose: () => undefined }),
  };
  server.registerChannel("escode-agent", agentChannel);
}
server.ready();
process.stdin.on("end", () => process.exit(0));
