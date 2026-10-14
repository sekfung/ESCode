import { describe, expect, it } from "vitest";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { WebSocket, type RawData } from "ws";
import { Emitter, SocketProtocol, VSBuffer, ChannelClient, type ISocket } from "@escode/rpc";
import { connectViaProtocol } from "@escode/client";

// docs/specs/rust-u2-host-skeleton.md §3.4 的「TS 接线」证明：**真实 `@escode/client`**
// 经 `@escode/rpc` 协议连 Rust Host 的 RPC 面（WebSocket），调用 `settingService` 成功，
// 并对未迁移的 channel/method 收到显式 NotImplemented。
//
// 链路：spawn escode-host → 控制消息 rpcListen{bind,home} → 得到 ws://addr/ws →
//       SocketProtocol(WS) → connectViaProtocol → services.settingService。

const hostBinary = resolve(
  `apps/escode-host/target/debug/escode-host${process.platform === "win32" ? ".exe" : ""}`,
);

function wrapNodeWebSocket(socket: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  socket.on("message", (raw: RawData) => {
    const data = Array.isArray(raw)
      ? Buffer.concat(raw)
      : Buffer.isBuffer(raw)
        ? raw
        : Buffer.from(raw);
    onData.fire(VSBuffer.wrap(new Uint8Array(data)));
  });
  socket.on("close", () => {
    onClose.fire();
    onEnd.fire();
  });
  socket.on("error", () => {
    onClose.fire();
    onEnd.fire();
  });
  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer) {
      if (socket.readyState === WebSocket.OPEN) socket.send(buffer.buffer);
    },
    end() {
      socket.close();
    },
    drain: async () => {},
    dispose() {
      socket.close();
    },
  };
}

interface HostProcess {
  child: ChildProcessWithoutNullStreams;
  send(message: Record<string, unknown>): void;
  next(): Promise<Record<string, unknown>>;
  stop(): Promise<void>;
}

function startHost(home: string): HostProcess {
  const child = spawn(hostBinary, [], {
    stdio: ["pipe", "pipe", "inherit"],
    env: { ...process.env, HOME: home, USERPROFILE: home },
  });
  let buffer = "";
  const pending: Array<(value: Record<string, unknown>) => void> = [];
  const queue: Record<string, unknown>[] = [];
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    buffer += chunk;
    let newline: number;
    while ((newline = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      if (!line.trim()) continue;
      const message = JSON.parse(line) as Record<string, unknown>;
      const resolveNext = pending.shift();
      if (resolveNext) resolveNext(message);
      else queue.push(message);
    }
  });
  return {
    child,
    send(message) {
      child.stdin.write(`${JSON.stringify(message)}\n`);
    },
    next() {
      const queued = queue.shift();
      if (queued) return Promise.resolve(queued);
      return new Promise((resolveNext) => pending.push(resolveNext));
    },
    async stop() {
      child.stdin.end();
      await new Promise<void>((done) => child.once("close", () => done()));
    },
  };
}

async function nextMeaningful(host: HostProcess): Promise<Record<string, unknown>> {
  for (let i = 0; i < 20; i += 1) {
    const message = await host.next();
    if (message.kind !== "logBatch") return message;
  }
  throw new Error("no meaningful control frame");
}

function connectRpc(url: string): Promise<ReturnType<typeof connectViaProtocol>> {
  return new Promise((resolveServices, reject) => {
    const socket = new WebSocket(url);
    socket.once("error", reject);
    socket.once("open", () => {
      resolveServices(connectViaProtocol(new SocketProtocol(wrapNodeWebSocket(socket))));
    });
  });
}

describe("rust host rpc surface", () => {
  it.skipIf(!existsSync(hostBinary))("serves the real client with settings and explicit gaps", async () => {
    const root = await mkdtemp(join(tmpdir(), "escode-rust-host-rpc-"));
    const home = join(root, "home");
    const host = startHost(home);
    try {
      const ready = await nextMeaningful(host);
      expect(ready.kind).toBe("hostReady");

      // Main 让 Host 开 RPC 面（设置根目录由 Main 指定）。
      host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home });
      const listening = await nextMeaningful(host);
      expect(listening.kind).toBe("rpcListening");
      const addr = String(listening.addr);

      // 真实客户端：connectViaProtocol（与 desktop 连接远端 Host 同一条路径）。
      const services = await connectRpc(`ws://${addr}/ws`);
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
      await expect(
        services.settingService.updateDataBaseDir(join(root, "data")),
      ).rejects.toThrow(/does not implement `setting\.updateDataBaseDir`/);

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

      // 写面转发：closeSession 按运行时语义返回「条件关闭成立」（未加载会话为 true）。
      const closed = (await services.escodeSessionService.closeSession({
        workspacePath,
        workspaceIdentity: workspacePath,
        sessionId: "sess_missing",
      } as never)) as boolean;
      expect(closed).toBe(true);

      // 未迁移的写方法（需 Host 侧特性门/draft 注册表）：显式 NotImplemented。
      await expect(
        services.escodeSessionService.createSession({
          workspacePath,
          workspaceIdentity: workspacePath,
        } as never),
      ).rejects.toThrow(/does not implement `escode-session.createSession`/);

      // 未迁移的 channel：显式 NotImplemented，且消息里带已实现清单。
      const client = new ChannelClient(new SocketProtocol(wrapNodeWebSocket(new WebSocket(`ws://${addr}/ws`))));
      await expect(client.getChannel("workspace").call("query", {})).rejects.toThrow(
        /does not implement channel `workspace` yet .*setting/,
      );
    } finally {
      await host.stop();
    }
  });
});
