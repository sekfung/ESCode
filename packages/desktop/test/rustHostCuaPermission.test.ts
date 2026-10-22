import { afterEach, describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { ChannelClient, Emitter, SocketProtocol, VSBuffer } from "@escode/rpc";
import { connectViaProtocol } from "@escode/client";
import {
  bridgeRustHostPort,
  startRustHost,
  type HostPortLike,
  type RustHostHandle,
} from "../src/main/rustHostBridge.js";

// `cua-permission`（docs/specs/rust-u2-cua-permission.md）：真实 Host → mainRequest → Main 形态的
// 返回值/参数整形。Main 侧的真实 Helper 生命周期不在单测里启动。

const hostBinary = resolve(
  `apps/escode-host/target/debug/escode-host${process.platform === "win32" ? ".exe" : ""}`,
);

describe("rust host cua-permission channel", () => {
  let handle: RustHostHandle | undefined;

  afterEach(async () => {
    await handle?.stop();
    handle = undefined;
  });

  it.skipIf(!existsSync(hostBinary))(
    "forwards getStatus/restartHelper through mainRequest",
    async () => {
      const requests: { method: string; params: unknown }[] = [];
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-cua-"));
      handle = await startRustHost({
        binary: hostBinary,
        home: join(root, "home"),
        onMainRequest: async (request) => {
          requests.push(request);
          if (request.method === "cua-permission.getStatus") {
            return { available: false, reason: "idle", idle: true };
          }
          return { ok: true };
        },
      });

      const [rendererPort, mainPort] = createPortPair();
      const teardown = bridgeRustHostPort(handle, mainPort, "desktop-continuous");
      const services = connectViaProtocol(new SocketProtocol(wrapPort(rendererPort)));

      await expect(
        services.cuaPermissionService.getStatus("/w", "id", {
          includeFunctionalProbes: true,
        } as never),
      ).resolves.toEqual({ available: false, reason: "idle", idle: true });
      expect(requests[0]).toEqual({
        method: "cua-permission.getStatus",
        params: {
          workspacePath: "/w",
          workspaceIdentity: "id",
          options: { includeFunctionalProbes: true },
        },
      });

      await expect(
        services.cuaPermissionService.restartHelper(undefined, undefined, {
          reason: "permission_granted",
        } as never),
      ).resolves.toEqual({ ok: true });
      expect(requests[1]).toEqual({
        method: "cua-permission.restartHelper",
        params: { options: { reason: "permission_granted" } },
      });

      teardown();
    },
    30_000,
  );

  it.skipIf(!existsSync(hostBinary))(
    "reports an explicit gap when Main did not declare mainServices",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-cua-gap-"));
      handle = await startRustHost({
        binary: hostBinary,
        home: join(root, "home"),
      });
      const [rendererPort, mainPort] = createPortPair();
      const teardown = bridgeRustHostPort(handle, mainPort, "desktop-continuous");
      const client = new ChannelClient(new SocketProtocol(wrapPort(rendererPort)));
      await expect(client.getChannel("cua-permission").call("getStatus", "/w")).rejects.toThrow(
        /mainServices/,
      );
      teardown();
    },
    30_000,
  );
});

function createPortPair(): [HostPortLike, HostPortLike] {
  const listenersA: Array<(event: { data: unknown }) => void> = [];
  const listenersB: Array<(event: { data: unknown }) => void> = [];
  const portA: HostPortLike = {
    on: (_event, listener) => listenersA.push(listener as (event: { data: unknown }) => void),
    postMessage: (value) => {
      for (const listener of listenersB) listener({ data: value });
    },
    start: () => undefined,
    close: () => undefined,
  };
  const portB: HostPortLike = {
    on: (_event, listener) => listenersB.push(listener as (event: { data: unknown }) => void),
    postMessage: (value) => {
      for (const listener of listenersA) listener({ data: value });
    },
    start: () => undefined,
    close: () => undefined,
  };
  return [portA, portB];
}

function wrapPort(port: HostPortLike) {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  port.on("message", (event: { data?: unknown } | unknown) => {
    onData.fire(VSBuffer.wrap(toBytes(extract(event))));
  });
  port.start?.();
  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer: VSBuffer) {
      port.postMessage(buffer.buffer);
    },
    end() {
      port.close();
    },
    drain: async () => undefined,
    dispose() {
      port.close();
    },
  } as never;
}

function extract(event: { data?: unknown } | unknown): unknown {
  return event && typeof event === "object" && "data" in event
    ? (event as { data?: unknown }).data
    : event;
}

function toBytes(value: unknown): Uint8Array {
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  if (Buffer.isBuffer(value)) return new Uint8Array(value);
  throw new Error(`unexpected port payload: ${typeof value}`);
}
