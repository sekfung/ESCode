import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, stat, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { CircuitIR } from "@escode/shared/circuit";
import { createFsCircuitStore } from "../../src/circuit/adapters/fsCircuitStore.js";
import { createFsSourceWatcher } from "../../src/circuit/adapters/sourceWatcher.js";
import { createWorkspaceSource } from "../../src/circuit/adapters/workspaceFiles.js";
import type { CircuitCorrectionRecord, StoredCircuit } from "../../src/circuit/app/ports.js";
import type { CircuitLayout } from "../../src/circuit/contract.js";

// 适配器测试统一用临时目录：store 注入 rootDir，source/watcher 用真实 workspace。
const cleanup: Array<() => void | Promise<void>> = [];
afterEach(async () => {
  for (const dispose of cleanup.splice(0).reverse()) await dispose();
});

async function makeTempDir(prefix: string): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), prefix));
  // 先入队的清理最后执行：watcher 先 dispose，再删目录（Windows 上目录句柄未释放时 rm 会 EBUSY）。
  cleanup.unshift(() => rm(dir, { recursive: true, force: true }));
  return dir;
}

function hash12(value: string): string {
  return createHash("sha256").update(value).digest("hex").slice(0, 12);
}

function makeIr(revision: number, irId = "ir-1"): CircuitIR {
  return {
    schemaVersion: 1,
    irId,
    revision,
    source: {
      kind: "kicad_sch",
      path: "hardware/demo.kicad_sch",
      contentHash: "a".repeat(64),
    },
    stale: false,
    components: [],
    nets: [],
    blocks: [],
    diagnostics: [],
  };
}

function makeLayout(irId: string, revision: number): CircuitLayout {
  return { irId, revision, sheets: [] };
}

function makeCorrection(revision: number): CircuitCorrectionRecord {
  return {
    revision,
    appliedAt: "2026-10-12T00:00:00.000Z",
    author: { kind: "user", sessionId: "session-1" },
    target: { kind: "element", id: "c1" },
    op: { op: "update-component", componentId: "c1", ref: "R1" },
  };
}

const identityKey = "workspace-a";
const sourcePath = "hardware/demo.kicad_sch";

function storeDir(rootDir: string, source = sourcePath, identity = identityKey): string {
  return join(rootDir, hash12(identity), hash12(source));
}

describe("电路 IR 文件存储适配器", () => {
  it("写入后能读回 IR、修正记录与渲染几何", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const store = createFsCircuitStore({ rootDir: root });
    const record: StoredCircuit = {
      irId: "ir-1",
      ir: makeIr(4),
      layout: makeLayout("ir-1", 4),
      corrections: [{ ...makeCorrection(4), orphaned: true }],
    };

    await store.write({ identityKey, sourcePath, record });

    await expect(store.read({ identityKey, sourcePath })).resolves.toEqual(record);
    // 目录布局：<rootDir>/<sha256(identityKey) 前 12 位>/<sha256(sourcePath) 前 12 位>/（§3.2）
    await expect(stat(join(storeDir(root), "ir.json"))).resolves.toBeTruthy();
    // ir.json 存的就是 IR 本体（提交点），corrections.json 存修正记录数组。
    const irText = await readFile(join(storeDir(root), "ir.json"), "utf-8");
    expect(JSON.parse(irText)).toEqual(makeIr(4));
    expect(JSON.parse(await readFile(join(storeDir(root), "corrections.json"), "utf-8"))).toEqual([
      { ...makeCorrection(4), orphaned: true },
    ]);
    await expect(stat(join(storeDir(root), "layout.json"))).resolves.toBeTruthy();
    await expect(stat(join(storeDir(root), "corrections.json"))).resolves.toBeTruthy();
  });

  it("目录或 ir.json 不存在时读回 undefined", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const store = createFsCircuitStore({ rootDir: root });
    await expect(store.read({ identityKey, sourcePath })).resolves.toBeUndefined();

    await mkdir(storeDir(root), { recursive: true });
    await expect(store.read({ identityKey, sourcePath })).resolves.toBeUndefined();
  });

  it("corrections.json 缺失时读回空数组", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const store = createFsCircuitStore({ rootDir: root });
    await store.write({
      identityKey,
      sourcePath,
      record: { irId: "ir-1", ir: makeIr(1), corrections: [] },
    });
    await unlink(join(storeDir(root), "corrections.json"));

    const read = await store.read({ identityKey, sourcePath });
    expect(read?.corrections).toEqual([]);
  });

  it("ir.json 不是合法 JSON 或结构不符合 schema 时抛 invalid_ir", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const store = createFsCircuitStore({ rootDir: root });
    await store.write({
      identityKey,
      sourcePath,
      record: { irId: "ir-1", ir: makeIr(1), corrections: [] },
    });

    await writeFile(join(storeDir(root), "ir.json"), "{ 这不是 JSON", "utf-8");
    await expect(store.read({ identityKey, sourcePath })).rejects.toThrow(/^invalid_ir: /u);

    // 合法 JSON 但 schemaVersion 非法：同样必须报 invalid_ir，不能当空 IR。
    await writeFile(
      join(storeDir(root), "ir.json"),
      JSON.stringify({ ...makeIr(1), schemaVersion: 2 }),
      "utf-8",
    );
    await expect(store.read({ identityKey, sourcePath })).rejects.toThrow(/^invalid_ir: /u);
  });

  it("layout 的 irId / revision 与 IR 不一致时读回不含 layout，corrections 解析失败抛 invalid_ir", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const store = createFsCircuitStore({ rootDir: root });

    // revision 落后（提交点规则：ir.json 是唯一提交点，旧几何不能交给渲染层）。
    await store.write({
      identityKey,
      sourcePath,
      record: {
        irId: "ir-1",
        ir: makeIr(5),
        layout: makeLayout("ir-1", 4),
        corrections: [],
      },
    });
    expect(await store.read({ identityKey, sourcePath })).toEqual({
      irId: "ir-1",
      ir: makeIr(5),
      corrections: [],
    });

    // revision 相同但属于另一份 IR（例如图片/PDF 内容变化后重新生成 irId）。
    await writeFile(join(storeDir(root), "layout.json"), JSON.stringify(makeLayout("ir-2", 5)));
    expect((await store.read({ identityKey, sourcePath }))?.layout).toBeUndefined();

    // layout.json 内容损坏。
    await writeFile(join(storeDir(root), "layout.json"), "{ oops", "utf-8");
    await expect(store.read({ identityKey, sourcePath })).rejects.toThrow(/^invalid_ir: /u);

    // corrections.json 损坏不能静默丢弃。
    await unlink(join(storeDir(root), "layout.json"));
    await writeFile(join(storeDir(root), "corrections.json"), "{ oops", "utf-8");
    await expect(store.read({ identityKey, sourcePath })).rejects.toThrow(/^invalid_ir: /u);
  });

  it("按 irId 能分别反查不同源文件的记录，未知 irId 返回 undefined", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const store = createFsCircuitStore({ rootDir: root });
    const first: StoredCircuit = { irId: "ir-a", ir: makeIr(1, "ir-a"), corrections: [] };
    const second: StoredCircuit = {
      irId: "ir-b",
      ir: makeIr(2, "ir-b"),
      corrections: [makeCorrection(2)],
    };
    await store.write({ identityKey, sourcePath: "hardware/a.kicad_sch", record: first });
    await store.write({ identityKey, sourcePath: "hardware/b.kicad_sch", record: second });

    await expect(store.findByIrId({ identityKey, irId: "ir-a" })).resolves.toEqual(first);
    await expect(store.findByIrId({ identityKey, irId: "ir-b" })).resolves.toEqual(second);
    await expect(store.findByIrId({ identityKey, irId: "ir-unknown" })).resolves.toBeUndefined();
    // 另一 workspace 的同名 irId 不串味。
    await expect(
      store.findByIrId({ identityKey: "workspace-b", irId: "ir-a" }),
    ).resolves.toBeUndefined();

    // 一份损坏的 ir.json 只让它自己查不出来，不能影响其它记录的匹配。
    await writeFile(join(storeDir(root, "hardware/a.kicad_sch"), "ir.json"), "{ oops", "utf-8");
    await expect(store.findByIrId({ identityKey, irId: "ir-b" })).resolves.toEqual(second);
    await expect(store.findByIrId({ identityKey, irId: "ir-a" })).resolves.toBeUndefined();
  });

  it("写入不含 layout 的记录时删除旧 layout.json", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const store = createFsCircuitStore({ rootDir: root });
    await store.write({
      identityKey,
      sourcePath,
      record: {
        irId: "ir-1",
        ir: makeIr(1),
        layout: makeLayout("ir-1", 1),
        corrections: [],
      },
    });

    await store.write({
      identityKey,
      sourcePath,
      record: { irId: "ir-1", ir: makeIr(2), corrections: [makeCorrection(2)] },
    });

    const read = await store.read({ identityKey, sourcePath });
    expect(read?.layout).toBeUndefined();
    expect(read?.corrections).toHaveLength(1);
    await expect(stat(join(storeDir(root), "layout.json"))).rejects.toThrow(/ENOENT/u);
  });
});

describe("workspace 源文件读取适配器", () => {
  it("读取文本并给出稳定的 sha256 内容哈希", async () => {
    const root = await makeTempDir("circuit-adapters-");
    await mkdir(join(root, "hardware"), { recursive: true });
    await writeFile(join(root, "hardware", "demo.kicad_sch"), "hello 电路", "utf-8");
    const source = createWorkspaceSource();

    await expect(
      source.readText({ workspacePath: root, relativePath: "hardware/demo.kicad_sch" }),
    ).resolves.toBe("hello 电路");
    const expected = createHash("sha256").update("hello 电路").digest("hex");
    expect(source.hashContent("hello 电路")).toBe(expected);
    expect(source.hashContent("hello 电路")).toBe(source.hashContent("hello 电路"));
    await expect(
      source.exists({ workspacePath: root, relativePath: "hardware/demo.kicad_sch" }),
    ).resolves.toBe(true);
  });

  it("源文件不存在时抛 not_found，越界路径抛 io_error 且 exists 返回 false", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const outside = await makeTempDir("circuit-adapters-outside-");
    await mkdir(join(root, "hardware"), { recursive: true });
    await writeFile(join(outside, "outside.txt"), "secret", "utf-8");
    const source = createWorkspaceSource();

    await expect(
      source.readText({ workspacePath: root, relativePath: "hardware/missing.kicad_sch" }),
    ).rejects.toThrow(/^not_found: /u);

    // 越界文件真实存在，必须因路径校验（而不是 ENOENT）被拒绝。
    const traversal = "../outside.txt";
    await expect(source.readText({ workspacePath: root, relativePath: traversal })).rejects.toThrow(
      /^io_error: /u,
    );
    await expect(source.exists({ workspacePath: root, relativePath: traversal })).resolves.toBe(
      false,
    );

    for (const absolute of [
      join(root, "hardware", "demo.kicad_sch"),
      "C:\\Windows\\win.ini",
      "\\\\server\\share\\file.txt",
      "hardware/../../outside.txt",
    ]) {
      await expect(
        source.readText({ workspacePath: root, relativePath: absolute }),
      ).rejects.toThrow(/^io_error: /u);
    }
  });

  it("超过 16 MiB 的源文件抛 too_large", async () => {
    const root = await makeTempDir("circuit-adapters-");
    await mkdir(join(root, "hardware"), { recursive: true });
    await writeFile(join(root, "hardware", "big.bin"), Buffer.alloc(16 * 1024 * 1024 + 1));
    const source = createWorkspaceSource();

    await expect(
      source.readText({ workspacePath: root, relativePath: "hardware/big.bin" }),
    ).rejects.toThrow(/^too_large: /u);
  });
});

function sleep(ms: number): Promise<void> {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

/** 轮询等待监听回调；给 5s 上限，避免 fs.watch 未触发时测试挂死。 */
async function waitFor(predicate: () => boolean, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("等待源文件监听回调超时");
    await sleep(25);
  }
}

describe("电路源文件监听适配器", () => {
  it("真实写入文件后收到 onChange 回调", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const file = join(root, "demo.kicad_sch");
    await writeFile(file, "v1", "utf-8");
    const watcher = createFsSourceWatcher();
    let count = 0;

    const handle = await watcher.watch({
      workspacePath: root,
      relativePath: "demo.kicad_sch",
      onChange: () => {
        count += 1;
      },
    });
    cleanup.unshift(() => handle.dispose());
    await sleep(100);

    await writeFile(file, "v2", "utf-8");
    await waitFor(() => count > 0);
    expect(count).toBeGreaterThan(0);
  });

  it("文件被删除并重新创建后仍能收到回调", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const file = join(root, "demo.kicad_sch");
    await writeFile(file, "v1", "utf-8");
    const watcher = createFsSourceWatcher({ debounceMs: 20 });
    let count = 0;

    const handle = await watcher.watch({
      workspacePath: root,
      relativePath: "demo.kicad_sch",
      onChange: () => {
        count += 1;
      },
    });
    cleanup.unshift(() => handle.dispose());
    await sleep(100);

    await unlink(file);
    await waitFor(() => count > 0);
    const afterDelete = count;

    await writeFile(file, "recreated", "utf-8");
    await waitFor(() => count > afterDelete);
  });

  it("dispose 之后不再回调", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const file = join(root, "demo.kicad_sch");
    await writeFile(file, "v1", "utf-8");
    const watcher = createFsSourceWatcher({ debounceMs: 20 });
    let count = 0;

    const handle = await watcher.watch({
      workspacePath: root,
      relativePath: "demo.kicad_sch",
      onChange: () => {
        count += 1;
      },
    });
    await sleep(100);
    await writeFile(file, "v2", "utf-8");
    await waitFor(() => count > 0);

    handle.dispose();
    const before = count;
    await writeFile(file, "v3", "utf-8");
    await sleep(400);
    expect(count).toBe(before);
  });

  it("父目录不存在时抛 not_found", async () => {
    const root = await makeTempDir("circuit-adapters-");
    const watcher = createFsSourceWatcher();
    await expect(
      watcher.watch({
        workspacePath: root,
        relativePath: "missing/demo.kicad_sch",
        onChange: () => undefined,
      }),
    ).rejects.toThrow(/^not_found: /u);
  });
});
