import { spawn, type ChildProcess } from "node:child_process";
import { randomUUID } from "node:crypto";
import { access, mkdir, realpath, rm } from "node:fs/promises";
import { connect, createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

const START_TIMEOUT_MS = 60_000;

export interface LocalESCodeServerOptions {
  /** 复用固定端口时用于“断开 → 原位重连”场景；缺省随机空闲端口。 */
  port?: number;
  /** 复用同一数据目录表示同一个 Source Server 身份。 */
  dataBaseDir?: string;
  workspacePath?: string;
  serverId?: string;
}

export interface LocalESCodeServer {
  url: string;
  token: string;
  workspacePath: string;
  serverId: string;
  port: number;
  /** 停掉进程但保留数据目录/端口，用于证明 Rust Host 的断线与原位重连。 */
  stopProcess(): Promise<void>;
  /** 按同一身份与端口重新启动，模拟服务端恢复。 */
  startProcess(): Promise<void>;
  stop(): Promise<void>;
}

/**
 * 真实 `escode-server` fixture（spec §12.6.7-4/5/6）：Desktop E2E 必须连接真实 HTTP/WS 服务，
 * 不能用 in-process fake 代替，否则 Rust Host 的 info 探测、capability 与帧泵都没有被覆盖。
 *
 * 客户端（Rust Host）连的是稳定的代理端口，后端 server 进程每次重启换一个内部端口：
 * 「断线 → 原位重连」对客户端仍然成立（URL/端口/身份不变），但不再依赖各平台
 * “进程退出后立刻复用监听端口”的行为——Windows 上 TIME_WAIT 会让同端口重建绑定失败，
 * 那是测试环境差异，不是 Desktop 的恢复语义。
 */
export async function startLocalESCodeServer(
  options: LocalESCodeServerOptions = {},
): Promise<LocalESCodeServer> {
  const entry = await resolveServerEntry();
  const publicPort = options.port ?? (await allocateFreePort());
  // Windows 的 tmpdir() 常含 8.3 短名（如 `C:\Users\RUNNER~1\...`）。server 会对 workspace
  // 递归 fs.watch，libuv 的 fs-event 用短名目录与 OS 回报的长名前缀比较，命中不一致时直接
  // 断言退出（`_wcsnicmp(filename, dir, dirlen)`，exit 3221226505）。macOS 同理：`/var` 是
  // `/private/var` 的符号链接。先 realpath 成规范长路径，避免把平台路径差异当成产品缺陷。
  const tempRoot = await realpath(tmpdir()).catch(() => tmpdir());
  const workspacePath =
    options.workspacePath ?? join(tempRoot, `escode-e2e-server-${randomUUID()}`, "workspace");
  const dataBaseDir =
    options.dataBaseDir ?? join(tempRoot, `escode-e2e-server-${randomUUID()}`, "home");
  // settings/Profile schema 要求 sourceServerId 是 UUID；server 的 info.serverId 直接投影为它。
  const serverId = options.serverId ?? randomUUID();
  const token = `e2e-token-${randomUUID()}`;
  await mkdir(workspacePath, { recursive: true });
  await mkdir(dataBaseDir, { recursive: true });

  const url = `http://127.0.0.1:${publicPort}`;
  let child: ChildProcess | null = null;
  let backendPort: number | null = null;
  let proxy: Server | null = null;
  const proxiedSockets = new Set<Socket>();

  function trackSocket(socket: Socket): void {
    proxiedSockets.add(socket);
    socket.once("close", () => proxiedSockets.delete(socket));
  }

  async function startProxy(): Promise<void> {
    const server = createServer((client) => {
      trackSocket(client);
      client.on("error", () => client.destroy());
      const target = backendPort;
      if (target === null) {
        // 后端停机期间接受连接后立刻断开，让客户端按“服务不可用”处理并在恢复后重连。
        client.destroy();
        return;
      }
      const upstream = connect(target, "127.0.0.1");
      trackSocket(upstream);
      upstream.on("error", () => {
        client.destroy();
        upstream.destroy();
      });
      client.on("error", () => upstream.destroy());
      client.pipe(upstream).pipe(client);
    });
    await new Promise<void>((settle, reject) => {
      server.once("error", reject);
      server.listen(publicPort, "127.0.0.1", () => settle());
    });
    proxy = server;
  }

  function destroyProxiedSockets(): void {
    for (const socket of proxiedSockets) {
      socket.destroy();
    }
    proxiedSockets.clear();
  }

  async function stopProxy(): Promise<void> {
    const server = proxy;
    proxy = null;
    if (!server) return;
    destroyProxiedSockets();
    await new Promise<void>((settle) => server.close(() => settle()));
  }

  async function launch(): Promise<void> {
    backendPort = await allocateFreePort();
    child = spawn(process.execPath, [entry], {
      cwd: dirname(entry),
      env: {
        ...process.env,
        PORT: String(backendPort),
        HOST: "127.0.0.1",
        ESCODE_SERVER_HOST: "127.0.0.1",
        ESCODE_SERVER_AUTH_TOKEN: token,
        ESCODE_SERVER_ID: serverId,
        ESCODE_SERVER_NAME: "Local E2E Server",
        ESCODE_SERVER_WORKSPACE: workspacePath,
        ESCODE_DATA_BASE_DIR: dataBaseDir,
        // 远端 Source Server 必须注册 provider-provisioning-target；缺少该权威模式会让
        // Rust Host 的首连 provisioning 屏障把真实 server 判成 capability-unavailable。
        ESCODE_SERVICE_AUTHORITY_MODE: "desktop-attached-remote",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stderr: string[] = [];
    child.stderr?.on("data", (chunk: Buffer) => {
      if (stderr.length < 40) stderr.push(chunk.toString().trim());
    });
    child.once("exit", (code) => {
      if (code !== 0 && code !== null) {
        stderr.push(`escode-server exited with code ${String(code)}`);
      }
    });
    await waitForServerInfo(url, token, serverId, stderr);
  }

  async function waitForServerInfo(
    baseUrl: string,
    authToken: string,
    expectedServerId: string,
    stderr: readonly string[],
  ): Promise<void> {
    const deadline = Date.now() + START_TIMEOUT_MS;
    while (Date.now() < deadline) {
      try {
        const response = await fetch(
          `${baseUrl}/api/server-info?token=${encodeURIComponent(authToken)}`,
        );
        if (response.ok) {
          const info = (await response.json()) as { serverId?: unknown };
          if (info.serverId === expectedServerId) return;
        }
      } catch {
        // 端口尚未 listen，继续轮询。
      }
      await new Promise((settle) => setTimeout(settle, 250));
    }
    throw new Error(
      `escode-server 未在 ${String(START_TIMEOUT_MS)}ms 内就绪 (${baseUrl}): ${stderr.join(" | ")}`,
    );
  }

  async function stopChild(): Promise<void> {
    const running = child;
    child = null;
    backendPort = null;
    if (running && running.exitCode === null) {
      const exited = new Promise<void>((settle) => running.once("exit", () => settle()));
      running.kill("SIGTERM");
      const forced = setTimeout(() => running.kill("SIGKILL"), 5_000);
      forced.unref?.();
      // 立刻断开代理连接：客户端不必等到 OS 回收管道才发现服务端已下线。
      destroyProxiedSockets();
      await exited;
      clearTimeout(forced);
      return;
    }
    destroyProxiedSockets();
  }

  await startProxy();
  await launch();

  return {
    url,
    token,
    workspacePath,
    serverId,
    port: publicPort,
    stopProcess: stopChild,
    startProcess: launch,
    async stop() {
      await stopChild();
      await stopProxy();
      await rm(dataBaseDir, { recursive: true, force: true });
      await rm(workspacePath, { recursive: true, force: true });
    },
  };
}

async function resolveServerEntry(): Promise<string> {
  const candidates = [
    resolve(process.cwd(), "packages/server/dist/entry-http.js"),
    resolve(process.cwd(), "../../packages/server/dist/entry-http.js"),
  ];
  for (const candidate of candidates) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // 继续尝试下一个候选根目录。
    }
  }
  throw new Error(
    `找不到 packages/server/dist/entry-http.js；先运行 pnpm --filter @escode/server build（cwd=${process.cwd()}）`,
  );
}

async function allocateFreePort(): Promise<number> {
  return new Promise<number>((settle, reject) => {
    const probe = createServer();
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      const port = typeof address === "object" && address ? address.port : 0;
      probe.close(() => (port > 0 ? settle(port) : reject(new Error("无法分配到空闲端口"))));
    });
  });
}
