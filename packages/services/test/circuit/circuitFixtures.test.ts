import { mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createCircuitService, type CircuitService } from "../../src/circuit/app/circuitService.js";
import { createFsCircuitStore } from "../../src/circuit/adapters/fsCircuitStore.js";
import { createFsSourceWatcher } from "../../src/circuit/adapters/sourceWatcher.js";
import { createWorkspaceSource } from "../../src/circuit/adapters/workspaceFiles.js";

/**
 * 用真实开源板子（packages/services/test/fixtures/circuit，来源与许可证见该目录 README）验证整条链路：
 * 解析 → 几何连通性 → IR 校验 → 持久化。真实文件由 KiCad 自己写出，能挡住人造片段覆盖不到的格式差异。
 */

const fixturesDir = fileURLToPath(new URL("../fixtures/circuit/", import.meta.url));

/**
 * 只枚举完整的 vendored 工程（目录里有 `.kicad_pro` 的那个 3 块开源板子）。
 * `rules/` 与 `firmware/` 是本仓库手写的最小片段（见该目录 README），不是完整板子，不在这里跑。
 */
async function listBoardFiles(): Promise<string[]> {
  const files: string[] = [];
  for (const board of await readdir(fixturesDir, { withFileTypes: true })) {
    if (!board.isDirectory()) continue;
    const entries = await readdir(join(fixturesDir, board.name));
    if (!entries.some((name) => name.endsWith(".kicad_pro"))) continue;
    for (const entry of entries)
      if (/\.kicad_(?:sch|pcb)$/u.test(entry)) files.push(`${board.name}/${entry}`);
  }
  return files.sort();
}

describe("真实开源板子端到端解析", () => {
  let service: CircuitService;
  let dataDir: string;

  beforeAll(async () => {
    dataDir = await mkdtemp(join(tmpdir(), "circuit-fixtures-"));
    service = createCircuitService({
      ports: {
        store: createFsCircuitStore({ rootDir: dataDir }),
        source: createWorkspaceSource(),
        watcher: createFsSourceWatcher(),
      },
    });
  });

  afterAll(async () => {
    service.dispose();
    await rm(dataDir, { recursive: true, force: true });
  });

  it("3 块板子的原理图与 PCB 都能解析出器件与网络，且不产生结构性错误", async () => {
    const files = await listBoardFiles();
    expect(files).toHaveLength(6);

    const summary: string[] = [];
    for (const file of files) {
      const view = await service.parse({ workspacePath: fixturesDir, path: file });
      const { componentCount, netCount, diagnostics } = view.summary;
      summary.push(
        `${file}: 器件 ${componentCount}，网络 ${netCount}，诊断 ${diagnostics.total}` +
          `（error ${diagnostics.error} / warning ${diagnostics.warning} / info ${diagnostics.info}）`,
      );
      // 每块板子都要有内容，且不能出现「引用不存在的元素」「位号重复」这类结构性问题。
      expect(componentCount, `${file} 器件数`).toBeGreaterThan(0);
      expect(netCount, `${file} 网络数`).toBeGreaterThan(0);
      const structural = view.diagnostics.filter(
        (diagnostic) =>
          diagnostic.code === "duplicate-ref" ||
          diagnostic.code === "unknown-component-reference" ||
          diagnostic.code === "unknown-net-reference",
      );
      expect(structural, `${file} 结构性诊断`).toEqual([]);

      const stored = await service.get({ workspacePath: fixturesDir, irId: view.summary.irId });
      expect(stored.summary.revision).toBe(1);
      expect(stored.summary.source.path).toBe(file);
    }
    // 输出解析规模，便于人工核对（评测报告与回归对比都用它）。
    console.log(`[circuit fixtures]\n${summary.join("\n")}`);
  }, 120_000);
});
