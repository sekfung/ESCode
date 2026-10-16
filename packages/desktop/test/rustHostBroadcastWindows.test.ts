import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdtemp, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-user-config-services.md §3：目标拓扑是「一个 App 一个 Host，所有窗口经 attachment 接入」，
// 所以跨窗口广播与 claim 仲裁都在 Host 进程内完成，不经 Main 的 BroadcastHub。
// 这里用同一个 Host 的多条连接（= 多个窗口）验证这一点；TS 单连接语义已由 rustHostUserConfig.test.ts 对拍。

type Message = { channel: string; payload?: unknown; sourceWindowId?: number };

const settle = (ms = 250) => new Promise((resolve) => setTimeout(resolve, ms));

describe("rust host broadcast across windows (one Host, many connections)", () => {
  it.skipIf(!existsSync(hostBinary))(
    "fans messages out to every window exactly once and arbitrates claims in-process",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-bcast-")));
      const home = join(base, "home");
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home });
        const listening = await nextMeaningful(host);
        const open = async () =>
          (await connectRpc(String(listening.addr), String(listening.secret))).broadcastService;
        const [a, b, c] = [await open(), await open(), await open()];

        const received: Record<string, Message[]> = { a: [], b: [], c: [] };
        const subscriptions = [
          a.onMessage((m) => received.a!.push(m as Message)),
          b.onMessage((m) => received.b!.push(m as Message)),
          c.onMessage((m) => received.c!.push(m as Message)),
        ];
        await settle();

        // 1) 发送窗口的消息到达所有窗口（含发送者自己，且各恰好一次），顺序保持。
        await a.send({ channel: "state:theme", payload: { mode: "dark" }, sourceWindowId: 1 });
        await b.send({ channel: "state:locale", payload: "en", sourceWindowId: 2 });
        await a.send({ channel: "state:theme", payload: { mode: "light" }, sourceWindowId: 1 });
        await settle();
        const expected: Message[] = [
          { channel: "state:theme", payload: { mode: "dark" }, sourceWindowId: 1 },
          { channel: "state:locale", payload: "en", sourceWindowId: 2 },
          { channel: "state:theme", payload: { mode: "light" }, sourceWindowId: 1 },
        ];
        expect(received).toEqual({ a: expected, b: expected, c: expected });

        // 2) claim 在进程内仲裁：同一 key 只有一个窗口拿到 lease，其余 busy；commit 之后是 committed。
        const first = await a.acquireClaim("notify:once");
        expect(first.status).toBe("acquired");
        const lease = (first as { lease: { key: string; token: string } }).lease;
        const contender = await b.acquireClaim("notify:once");
        expect(contender).toMatchObject({ status: "busy" });
        expect((contender as { retryAfterMs: number }).retryAfterMs).toBeLessThanOrEqual(250);
        // 别的窗口拿着错误 token 释放无效。
        await c.releaseClaim({ key: lease.key, token: "not-the-token" });
        expect(await b.acquireClaim("notify:once")).toMatchObject({ status: "busy" });
        await a.commitClaim(lease);
        expect(await b.acquireClaim("notify:once")).toEqual({ status: "committed" });
        expect(await c.acquireClaim("notify:once")).toEqual({ status: "committed" });

        // 3) 三个窗口同时抢同一个 key：恰好一个赢家。
        const racers = await Promise.all([
          a.tryClaim("race"),
          b.tryClaim("race"),
          c.tryClaim("race"),
        ]);
        expect(racers.filter(Boolean)).toHaveLength(1);

        // 4) 持有者释放后，其他窗口可以重新获得。
        const held = await b.acquireClaim("handoff");
        expect(held.status).toBe("acquired");
        expect(await c.acquireClaim("handoff")).toMatchObject({ status: "busy" });
        await b.releaseClaim((held as { lease: { key: string; token: string } }).lease);
        expect((await c.acquireClaim("handoff")).status).toBe("acquired");

        // 5) 窗口关闭（取消订阅/断开）后，其余窗口不受影响；新窗口不会收到历史消息。
        subscriptions[2]!.dispose();
        received.a!.length = received.b!.length = received.c!.length = 0;
        await a.send({ channel: "after-close", payload: 1 });
        await settle();
        expect(received.a).toHaveLength(1);
        expect(received.b).toHaveLength(1);
        expect(received.c).toHaveLength(0);
        const late = await open();
        const lateMessages: Message[] = [];
        const lateSubscription = late.onMessage((m) => lateMessages.push(m as Message));
        await settle();
        expect(lateMessages, "广播没有重放").toHaveLength(0);
        await b.send({ channel: "for-late", payload: 2 });
        await settle();
        expect(lateMessages).toHaveLength(1);
        lateSubscription.dispose();
        subscriptions[0]!.dispose();
        subscriptions[1]!.dispose();
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
