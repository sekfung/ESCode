import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-host-resilience.md §2.4：runtime 崩溃时，host-rpc 必须把
// `RuntimeClosed` 原样映射成 Renderer 可见的 `{name: "Error", message: "ESCode agent transport closed: stdout_closed"}`
// （不是 `Fatal`，也不是请求自身的超时）。这里用真实 `@escode/client` 走完整链路：
//   child runtime → forward.rs → RPC PromiseError → ChannelClient 重建 Error。
//
// 让请求确定停在 runtime 内：先 SIGSTOP CLI 子进程（它不再读 stdin / 应答），再发一个转发请求；
// 随后 SIGKILL，实例帧泵读到 stdout EOF 并立即失败在飞请求。SIGSTOP/SIGKILL 只在 POSIX 可用，
// Windows 的 TerminateProcess 分支由 host-runtime 的 crash_isolation.rs 覆盖。

const cliBinary = resolve(
  `apps/escode-cli-rust/target/debug/escode-cli-rust${process.platform === "win32" ? ".exe" : ""}`,
);

interface LifecycleEvent {
  state: "available" | "unavailable";
  workspaceKey: string;
  runtimeIdentity: {
    generation: number;
    identity: string;
    processId?: number;
  };
}

async function waitFor(predicate: () => boolean, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("condition not reached in time");
    await new Promise((resolveTick) => setTimeout(resolveTick, 20));
  }
}

describe("rust host rpc runtime crash", () => {
  it.skipIf(!existsSync(hostBinary) || !existsSync(cliBinary) || process.platform === "win32")(
    "rejects the in-flight real client request with the Node transport-closed error",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-crash-"));
      const home = join(root, "home");
      const workspacePath = join(root, "workspace");
      await mkdir(workspacePath, { recursive: true });
      const host = startHost(home, {
        ESCODE_HOST_RUNTIME_LINK: "child",
        ESCODE_HOST_RUNTIME_BINARY: cliBinary,
      });
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({
          kind: "rpcListen",
          bind: "127.0.0.1:0",
          home,
          dataBaseDir: join(root, "data"),
        });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const target = { workspacePath, workspaceIdentity: workspacePath };

        // 先订阅生命周期，再启动实例：否则 available（含 processId）会错过。
        const events: LifecycleEvent[] = [];
        const listener = services.escodeAgentService.onAgentRuntimeLifecycle?.((event) =>
          events.push(event as unknown as LifecycleEvent),
        );
        const initialized = (await services.escodeSessionService.initializeWorkspace(
          target as never,
        )) as { available: boolean };
        expect(initialized.available).toBe(true);
        await waitFor(() => events.some((event) => event.state === "available"));
        const available = events.find((event) => event.state === "available")!;
        const pid = available.runtimeIdentity.processId;
        expect(pid).toBeTypeOf("number");

        // 停住 runtime 后再发请求：Host 会登记为在飞请求并写入子进程 stdin，但永远等不到应答。
        process.kill(pid!, "SIGSTOP");
        const pending = services.escodeSessionService.listSessions(target as never);
        // 给 Host 的读循环登记请求并写入 stdin 的时间；子进程已停，不存在“先答完”的竞态。
        await new Promise((resolveTick) => setTimeout(resolveTick, 300));

        // stdout EOF → 帧泵清空在飞请求 → forward.rs 映射为 name=Error 的 RpcError。
        process.kill(pid!, "SIGKILL");
        const error = await pending.catch((reason: unknown) => reason);
        expect(error).toBeInstanceOf(Error);
        expect((error as Error).name).toBe("Error");
        expect((error as Error).message).toBe("ESCode agent transport closed: stdout_closed");
        expect(String(error)).toBe("Error: ESCode agent transport closed: stdout_closed");

        // 同一崩溃也发布 unavailable（Renderer 据此让该代的订阅失效）。
        await waitFor(() => events.some((event) => event.state === "unavailable"));
        const unavailable = events.find((event) => event.state === "unavailable")!;
        expect(unavailable.workspaceKey).toBe(workspacePath);
        expect(unavailable.runtimeIdentity.generation).toBe(1);

        listener?.dispose();
      } finally {
        await host.stop();
        await rm(root, { recursive: true, force: true });
      }
    },
    60_000,
  );
});
