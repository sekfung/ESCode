import { afterEach, describe, expect, it, vi } from "vitest";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const mocks = vi.hoisted(() => ({
  markTasksStoragePrepared: vi.fn(),
  markLocalDatabaseStartupReady: vi.fn(),
}));

vi.mock("@escode/services/storage-startup", () => ({
  getTasksIndexDatabasePath: () => "/tmp/escode-tasks-index.sqlite",
  markTasksStoragePrepared: mocks.markTasksStoragePrepared,
}));
vi.mock("../src/main/databaseStartupRelay.js", () => ({
  markLocalDatabaseStartupReady: mocks.markLocalDatabaseStartupReady,
}));

import { createRustHostManager, type RustHostManager } from "../src/main/rustHostManager.js";
import { createRustHostTasksIndexPreparation } from "../src/main/rustHostStorageStartup.js";

// docs/specs/rust-u2-tasks-index.md §3.1：Rust Host 不建库，Main 必须在 spawn 前准备 tasks-index。

const managers: RustHostManager[] = [];

afterEach(async () => {
  for (const manager of managers.splice(0)) await manager.stop();
  vi.clearAllMocks();
});

describe("Rust host tasks-index preparation", () => {
  it("prepares once per process and marks the local database ready", async () => {
    const prepare = vi.fn().mockResolvedValue(undefined);
    const preparation = createRustHostTasksIndexPreparation({ prepare });

    await Promise.all([preparation(), preparation()]);
    await preparation();

    expect(prepare).toHaveBeenCalledTimes(1);
    expect(prepare).toHaveBeenCalledWith("/tmp/escode-tasks-index.sqlite");
    expect(mocks.markTasksStoragePrepared).toHaveBeenCalledWith("/tmp/escode-tasks-index.sqlite");
    expect(mocks.markLocalDatabaseStartupReady).toHaveBeenCalledTimes(1);
  });

  it("does not cache an unsuccessful preparation", async () => {
    const prepare = vi.fn().mockRejectedValueOnce(new Error("locked")).mockResolvedValueOnce();
    const preparation = createRustHostTasksIndexPreparation({ prepare });

    await expect(preparation()).rejects.toThrow("locked");
    expect(mocks.markLocalDatabaseStartupReady).not.toHaveBeenCalled();
    await preparation();

    expect(prepare).toHaveBeenCalledTimes(2);
    expect(mocks.markLocalDatabaseStartupReady).toHaveBeenCalledTimes(1);
  });

  it("rejects host startup with RUST_HOST_STORAGE_FAILED before spawning", async () => {
    const home = mkdtempSync(join(tmpdir(), "escode-rust-host-storage-"));
    const prepareStorage = vi.fn().mockRejectedValue(new Error("checksum mismatch"));
    const manager = createRustHostManager({
      binary: join(home, "no-such-escode-host"),
      home,
      prepareStorage,
    });
    managers.push(manager);

    await expect(manager.ensureStarted()).rejects.toMatchObject({
      name: "RustHostStartError",
      code: "RUST_HOST_STORAGE_FAILED",
      message: expect.stringContaining("checksum mismatch"),
    });
    expect(prepareStorage).toHaveBeenCalledTimes(1);
  });

  it("moves past preparation before reporting a missing binary", async () => {
    const home = mkdtempSync(join(tmpdir(), "escode-rust-host-storage-"));
    const prepareStorage = vi.fn().mockResolvedValue(undefined);
    const manager = createRustHostManager({
      binary: join(home, "no-such-escode-host"),
      home,
      prepareStorage,
    });
    managers.push(manager);

    await expect(manager.ensureStarted()).rejects.toMatchObject({
      code: "RUST_HOST_BINARY_MISSING",
    });
    expect(prepareStorage).toHaveBeenCalledTimes(1);
  });
});
