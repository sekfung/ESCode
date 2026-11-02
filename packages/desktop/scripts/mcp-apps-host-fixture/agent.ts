import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { createServer, type Socket } from "node:net";
import { createInterface } from "node:readline";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { ESCodeProtocolClient } from "../../../services/src/escode-agent/escodeProtocolClient.js";
import { ESCodeStdioTransport } from "../../../services/src/escode-agent/escodeStdioTransport.js";

export class Events<T> {
  readonly history: T[] = [];
  private waiters = new Set<{ match: (event: T) => boolean; resolve: (event: T) => void }>();
  emit(event: T) {
    this.history.push(event);
    for (const waiter of this.waiters)
      if (waiter.match(event)) {
        this.waiters.delete(waiter);
        waiter.resolve(event);
      }
  }
  wait(match: (event: T) => boolean, after = 0): Promise<T> {
    const existing = this.history.slice(after).find(match);
    if (existing) return Promise.resolve(existing);
    return new Promise((resolve) => this.waiters.add({ match, resolve }));
  }
}
export async function startAgent(
  root: string,
  node: string,
  cli: string,
  providerEnv: Record<string, string> = {},
) {
  const trace = (message: string) => {
    if (process.env.ESCODE_FIXTURE_TRACE) process.stderr.write(`[background:agent] ${message}\n`);
  };
  const events = new Events<any>();
  const approvals = new Events<any>();
  const notifications = new Events<any>();
  const sockets = new Map<string, Socket>();
  const socketSources = new Map<Socket, string>();
  const control = createServer((socket) => {
    createInterface({ input: socket }).on("line", (line) => {
      const event = JSON.parse(line);
      socketSources.set(socket, event.source);
      sockets.set(event.source, socket);
      events.emit(event);
    });
    socket.on("close", () => {
      const source = socketSources.get(socket);
      socketSources.delete(socket);
      if (source && sockets.get(source) === socket) {
        const previous = [...socketSources]
          .reverse()
          .find(([candidate, key]) => key === source && !candidate.destroyed);
        if (previous) sockets.set(source, previous[0]);
        else sockets.delete(source);
      }
    });
  });
  await new Promise<void>((resolve) => control.listen(0, "127.0.0.1", resolve));
  const port = (control.address() as { port: number }).port;
  const data = join(root, "agent-data");
  await mkdir(data, { recursive: true });
  // 修复：Windows 的 stdout/stderr 管道写是同步的。把 Agent 调试日志原样转到 CI 管道，
  // 当日志量较大或单行过长时可能反压主进程并让双方互等；Windows 下改为落盘，失败时由
  // 外层 dumpAgentLogs 读取尾部，Mac/Linux 保持实时输出。
  const stderrLogPath = join(data, "agent-stderr.log");
  const stderrLog =
    process.platform === "win32" ? createWriteStream(stderrLogPath, { flags: "a" }) : undefined;
  stderrLog?.on("error", () => undefined);
  let stderrBytes = 0;
  const forwardAgentStderr = (line: string) => {
    if (!stderrLog) {
      process.stderr.write(line + "\n");
      return;
    }
    // 防止异常刷屏把 E2E 临时目录写爆；超过上限后只保留前 32MB。
    if (stderrBytes >= 32 * 1024 * 1024) return;
    stderrBytes += Buffer.byteLength(line, "utf8") + 1;
    stderrLog.write(line + "\n");
  };
  // 修复：Windows 下只传 PATH 会让 Node/MCP 子进程缺少系统目录、临时目录和命令解释器信息，
  // Windows runner 上曾出现 session/create 在 runtime preferences 回包后无响应。这里只补
  // 运行时必需的系统变量，不展开完整 process.env，避免把 API key、认证和 Electron 标志带进夹具。
  const windowsSystemEnv =
    process.platform === "win32"
      ? {
          TEMP: process.env.TEMP,
          TMP: process.env.TMP,
          SystemRoot: process.env.SystemRoot,
          windir: process.env.windir,
          ComSpec: process.env.ComSpec,
          PATHEXT: process.env.PATHEXT,
          USERPROFILE: process.env.USERPROFILE,
          APPDATA: process.env.APPDATA,
          LOCALAPPDATA: process.env.LOCALAPPDATA,
          PROGRAMDATA: process.env.PROGRAMDATA,
          PROGRAMFILES: process.env.PROGRAMFILES,
          "PROGRAMFILES(X86)": process.env["PROGRAMFILES(X86)"],
          COMMONPROGRAMFILES: process.env.COMMONPROGRAMFILES,
          HOMEDRIVE: process.env.HOMEDRIVE,
          HOMEPATH: process.env.HOMEPATH,
          NUMBER_OF_PROCESSORS: process.env.NUMBER_OF_PROCESSORS,
          PROCESSOR_ARCHITECTURE: process.env.PROCESSOR_ARCHITECTURE,
        }
      : {};
  // 不继承 API key、真实用户认证及 Electron Node 标志，模型测试也只通过显式传入的隔离配置访问本地 fixture。
  const child = spawn(node, [cli, "app-server"], {
    cwd: data,
    env: {
      PATH: process.env.PATH,
      ...windowsSystemEnv,
      ESCODE_DATA_BASE_DIR: data,
      ESCODE_SESSION_DB: join(data, "session.sqlite"),
      ESCODE_STORAGE_DIR: data,
      LANG: "en_US.UTF-8",
      ...(process.env.ESCODE_FIXTURE_TRACE
        ? {
            ESCODE_FIXTURE_TRACE: process.env.ESCODE_FIXTURE_TRACE,
            ESCODE_LOG_LEVEL: "debug",
            ESCODE_LOG_DIR: join(data, "logs"),
            ESCODE_LOG_CONSOLE: "1",
          }
        : {}),
      ...providerEnv,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.on("error", (error) => trace(`child error ${String(error)}`));
  child.on("exit", (code, signal) => trace(`child exit code=${code} signal=${signal}`));
  const transport = new ESCodeStdioTransport(child, {
    onStderrLine: forwardAgentStderr,
  });
  const send = transport.send.bind(transport);
  transport.send = async (message) => {
    const kind = (message as { method?: string }).method ?? "response";
    trace(`send ${kind} bytes=${JSON.stringify(message).length}`);
    await send(message);
    trace(`send done ${kind}`);
  };
  transport.onMessage((message) => {
    const method = (message as { method?: string }).method ?? "response";
    const params = (message as { params?: Record<string, unknown> }).params;
    const state =
      method === "startup/storageState" && params
        ? ` phase=${String(params.phase)} sequence=${String(params.sequence)} elapsedMs=${String(params.elapsedMs)} error=${String(params.errorCode ?? "")}`
        : "";
    trace(`recv ${method} bytes=${JSON.stringify(message).length}${state}`);
  });
  const client = new ESCodeProtocolClient(transport, {
    requireStorageStartup: true,
    requestTimeoutMs: 15_000,
  });
  client.onNotification((event) => notifications.emit(event));
  client.onRequest((request) => {
    if (request.method === "session/requestRuntimePreferences") {
      trace("responding runtime preferences");
      void client
        .respond(request.id, {
          nativeSearchEnhancementsEnabled: false,
          memoryEnabled: false,
        })
        .then(
          () => trace("runtime preferences responded"),
          (error) => trace(`runtime preferences respond failed ${String(error)}`),
        );
    } else if (request.method === "interaction/requestPermission") approvals.emit(request);
    else
      void client.respondError(request.id, -32601, `Unexpected fixture request: ${request.method}`);
  });
  // 真实编辑器的长轮询为 20 秒，不能套夹具普通 RPC 的 15 秒期限（上游同款修正）。
  const call = async (method: string, params: unknown) => {
    if (process.env.ESCODE_FIXTURE_TRACE)
      process.stdout.write(`[background:agent] call ${method}\n`);
    const result = await client.request<any>(
      method as never,
      params,
      undefined,
      method === "mcp/uiCallTool" ? { timeoutMs: 180_000 } : undefined,
    );
    if (process.env.ESCODE_FIXTURE_TRACE)
      process.stdout.write(`[background:agent] call ${method} done\n`);
    return result;
  };
  return {
    call,
    client,
    events,
    approvals,
    notifications,
    respond: (request: any, decision: "allow" | "deny") => client.respond(request.id, { decision }),
    release: (source: string, key: string) =>
      sockets.get(source)!.write(JSON.stringify({ type: "release", key }) + "\n"),
    disconnect: (source: string) =>
      sockets.get(source)!.write(JSON.stringify({ type: "disconnect" }) + "\n"),
    notify: (source: string, uri: string) =>
      sockets.get(source)!.write(JSON.stringify({ type: "notify", uri }) + "\n"),
    async session(
      workspacePath: string,
      source: string,
      serverName = "fixture",
      workspaceIdentity?: string,
      demo?: { serverScript: string; resourceUri: string },
    ) {
      await mkdir(workspacePath, { recursive: true });
      const workspace = {
        workspacePath,
        workspaceKey: workspaceIdentity ?? workspacePath,
        ...(workspaceIdentity ? { workspaceIdentity } : {}),
      };
      const response = await call("session/create", {
        workspace,
        mode: "build",
        persistence: "deferred",
        titleGenerationEnabled: false,
        mcpServers: [
          {
            name: serverName,
            command: node,
            args: demo
              ? [demo.serverScript]
              : [join(root, "mcp-server.mjs"), String(port), join(root, "widget.html"), source],
            env: [],
            protocolVersion: "legacy",
            isolation: "session",
          },
        ],
      });
      return {
        workspace,
        sessionId: response.session.sessionId,
        pluginId: "fixture",
        serverName,
        ...(demo ? { resourceUri: demo.resourceUri } : {}),
      };
    },
    async uiSession(
      workspacePath: string,
      source: string,
      demo?: { serverScript: string; resourceUri: string; surface: string },
    ) {
      // PLM-UI-BACKGROUND-ADMISSION：工具必须来自插件 MCP server（plugin:<id>:<server>），
      // 才能覆盖 pluginId 归一化与“无卡片、无 conversation 订阅”的窗口级接收器路径。
      const pluginRoot = join(workspacePath, "editor-plugin");
      await mkdir(join(pluginRoot, ".escode-plugin"), { recursive: true });
      await writeFile(
        join(pluginRoot, ".escode-plugin", "plugin.json"),
        JSON.stringify({
          name: "background-editor",
          mcpServers: {
            editor: {
              command: node,
              args: demo
                ? [demo.serverScript]
                : [join(root, "mcp-server.mjs"), String(port), join(root, "widget.html"), source],
            },
          },
        }),
      );
      await writeFile(
        join(workspacePath, "escode.json"),
        JSON.stringify({ plugins: { dirs: [pluginRoot] } }),
      );
      const workspace = { workspacePath, workspaceKey: workspacePath };
      const response = await call("session/create", {
        workspace,
        mode: "yolo",
        persistence: "deferred",
        titleGenerationEnabled: false,
      });
      return {
        workspace,
        sessionId: response.session.sessionId,
        pluginId: "background-editor@inline",
        serverName: "plugin:background-editor:editor",
        ...(demo ? { resourceUri: demo.resourceUri, surface: demo.surface } : {}),
      };
    },
    async close() {
      await transport.disposeAndWait();
      client.dispose();
      stderrLog?.end();
      for (const socket of sockets.values()) socket.destroy();
      await new Promise<void>((resolve) => control.close(() => resolve()));
    },
  };
}
