import { EventEmitter } from "node:events";
import { describe, expect, it, vi } from "vitest";
import { RustHostStartError, startRustHost } from "../src/main/rustHostBridge.js";

const spawnMock = vi.hoisted(() => vi.fn());

vi.mock("node:child_process", () => ({
  spawn: spawnMock,
}));

function createFakeChild() {
  const child = new EventEmitter() as EventEmitter & {
    kill: ReturnType<typeof vi.fn>;
    stderr: EventEmitter;
    stdin: { write: ReturnType<typeof vi.fn> };
    stdout: EventEmitter & { setEncoding: ReturnType<typeof vi.fn> };
  };
  child.kill = vi.fn();
  child.stdin = { write: vi.fn(() => true) };
  child.stdout = Object.assign(new EventEmitter(), { setEncoding: vi.fn() });
  child.stderr = Object.assign(new EventEmitter(), { setEncoding: vi.fn() });
  // 二进制缺失：只验证 spawn 参数，不在这里跑真实握手。
  queueMicrotask(() => {
    child.emit("error", Object.assign(new Error("spawn ENOENT"), { code: "ENOENT" }));
  });
  return child;
}

describe("rust host spawn options", () => {
  it("hides the Windows console window so closing / Ctrl+C cannot terminate the Host", async () => {
    spawnMock.mockImplementation(() => createFakeChild());

    await expect(
      startRustHost({ binary: "escode-host", home: "/tmp/escode-host-home" }),
    ).rejects.toBeInstanceOf(RustHostStartError);

    expect(spawnMock).toHaveBeenCalledTimes(1);
    const [binary, args, options] = spawnMock.mock.calls[0] as [
      string,
      string[],
      Record<string, unknown>,
    ];
    expect(binary).toBe("escode-host");
    expect(args).toEqual([]);
    // escode-host 是 console 子系统程序：不隐藏时 Windows 会创建控制台窗口，
    // 关闭窗口或 Ctrl+C 会以 0xC000013A（STATUS_CONTROL_C_EXIT）杀掉 Host。
    expect(options).toMatchObject({
      stdio: ["pipe", "pipe", "inherit"],
      windowsHide: true,
      env: expect.objectContaining({ HOME: "/tmp/escode-host-home" }),
    });
  });
});
