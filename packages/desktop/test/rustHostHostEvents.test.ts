import { afterEach, describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { startRustHost, type RustHostHandle } from "../src/main/rustHostBridge.js";
import {
  requestExternalHostResourceUsage,
  resolveHostResourceUsageResult,
} from "../src/main/resourceManagerHostSampling.js";

// docs/specs/rust-u2-window-host.md §5-2：真实 Host 的 HostEvent（资源采样结果）经控制通道
// 到达 Main，并由真实 Main consumer（resourceManagerHostSampling 的 requestId 关联）消费。

const hostBinary = resolve(
  `apps/escode-host/target/debug/escode-host${process.platform === "win32" ? ".exe" : ""}`,
);

describe("rust host HostEvent round trip", () => {
  let handle: RustHostHandle | undefined;

  afterEach(async () => {
    await handle?.stop();
    handle = undefined;
  });

  it.skipIf(!existsSync(hostBinary))(
    "delivers a resource-usage result from the real Host to the Main consumer",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-event-"));
      handle = await startRustHost({
        binary: hostBinary,
        home: join(root, "home"),
        onHostEvent: (event) => {
          const record = event as { type?: string };
          if (record.type === "resource-usage-snapshot-result") {
            resolveHostResourceUsageResult("rust-host", event as never);
          }
        },
      });

      const processes = await requestExternalHostResourceUsage(
        "rust-host",
        (message) =>
          handle?.send({
            kind: message.type.includes("cancel")
              ? "resourceUsageSnapshotCancel"
              : "resourceUsageSnapshotRequest",
            requestId: message.requestId,
          }),
        10_000,
      );

      expect(processes.length).toBeGreaterThan(0);
      expect(processes.some((row) => row.pid === handle?.pid)).toBe(true);
      expect(processes.every((row) => row.groupKey === "host")).toBe(true);
    },
    30_000,
  );
});
