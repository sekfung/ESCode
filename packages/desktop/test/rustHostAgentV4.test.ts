import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-agent-v4.md §4：**真实 `@escode/client`**（`ProxyChannel.toService` + `ChannelClient`）
// 经 Rust Host 的事件面收 `EventFire`：静态事件 `onAgentRuntimeLifecycle`、动态事件
// `onDynamicSessionsIndexFrame(target)`，以及 V4 握手与 sessions-index 订阅。证明 Rust 侧
// `[204, id] + data` 编码、EventListen/EventDispose 与 TS 客户端互通（Rust 侧用例用的是手写编解码）。

async function waitFor(predicate: () => boolean, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("condition not reached in time");
    await new Promise((resolveTick) => setTimeout(resolveTick, 20));
  }
}

describe("rust host escode-agent v4 surface", () => {
  it.skipIf(!existsSync(hostBinary))(
    "delivers lifecycle and sessions-index frames to the real client",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-v4-"));
      const home = join(root, "home");
      const workspacePath = join(root, "workspace");
      await mkdir(workspacePath, { recursive: true });
      const host = startHost(home);
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
        const agent = services.escodeAgentService;
        const target = { workspacePath, workspaceIdentity: workspacePath };

        // 静态事件：实例启动即 available（TS `ESCodeAgentRuntimeLifecycleEvent` 形状）。
        const lifecycle: Array<Record<string, unknown>> = [];
        const lifecycleListener = agent.onAgentRuntimeLifecycle?.((event) =>
          lifecycle.push(event as unknown as Record<string, unknown>),
        );

        const hello = await agent.helloConversationV4();
        expect(hello.kind).toBe("hello");
        expect(hello.clientMode).toBe("desktop-continuous");
        expect(hello.connectionId).toMatch(/^host-rpc-/);
        await agent.initializeConversationV4({
          kind: "clientHello",
          protocolVersion: hello.protocolVersion,
          clientId: "client-real",
          appVersion: "test",
        });

        // 动态事件：先 listen 再 subscribe（与 Renderer 同序）。
        const frames: Array<{ subscriptionId: string; topic: string }> = [];
        const frameListener = agent.onDynamicSessionsIndexFrame(target)((frame) =>
          frames.push(frame as { subscriptionId: string; topic: string }),
        );
        const subscribed = await agent.subscribeSessionsIndexV4(target);
        const subscriptionId = subscribed.ack.subscriptionId;
        expect(subscriptionId).toBeTruthy();

        await waitFor(() => frames.some((frame) => frame.subscriptionId === subscriptionId));
        expect(frames.every((frame) => frame.topic === `sessions-index/${workspacePath}`)).toBe(
          true,
        );
        await waitFor(() => lifecycle.some((event) => event.state === "available"));
        const available = lifecycle.find((event) => event.state === "available")!;
        expect(available.workspaceKey).toBe(workspacePath);
        expect(available.workspacePath).toBe(workspacePath);
        expect((available.runtimeIdentity as { generation: number }).generation).toBe(1);

        await agent.unsubscribeSessionsIndexV4({ ...target, subscriptionId });
        await expect(
          agent.resyncSessionsIndexV4({ ...target, subscriptionId } as never),
        ).rejects.toThrow(/fault\.subscription\.notOwned/);
        frameListener.dispose();
        lifecycleListener?.dispose();
      } finally {
        await host.stop();
      }
    },
  );
});
