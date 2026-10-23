// Phase 3 golden freeze：rustHostFileWatcher.test.ts 的 TS `createFileWatcherService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：绝对临时根路径写成 `$FIXTURE_ROOT`；watcher id 不出现在事件载荷里，脚本只记录
// 相对 id 增量（不存在目录的失败也消耗 id），因此 id 归一化即该增量。事件在场景内按到达顺序
// 记录：每个场景的防抖窗口在 700 ms 静默后落定，且最多投递一个事件，顺序是确定性的。
import { deepEqual } from "node:assert/strict";
import {
  appendFile,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createFileWatcherService } from "../packages/services/src/node.ts";

type FileWatcherService = ReturnType<typeof createFileWatcherService>;
type Act = (dir: string) => Promise<void>;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** 防抖 150 ms + 余量。 */
const SETTLE_MS = 700;

async function scenario(
  service: FileWatcherService,
  root: string,
  name: string,
  recursive: boolean,
  act: Act,
  seed = true,
): Promise<unknown> {
  const dir = join(root, name);
  await mkdir(seed ? join(dir, "sub") : dir, { recursive: true });
  if (seed) {
    await writeFile(join(dir, "a.txt"), "1");
    await writeFile(join(dir, "sub", "n.txt"), "1");
  }
  const { id } = await service.watch({ path: dir, recursive });
  const events: unknown[] = [];
  service.onDynamicChange(id)((event) => events.push(event));
  await sleep(150);
  await act(dir);
  await sleep(SETTLE_MS);
  await service.unwatch({ id });
  return events;
}

/** 同一份脚本驱动一个文件监视服务，返回可比较的事件转录（与测试中的 drive 完全一致）。 */
async function drive(service: FileWatcherService, root: string): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};
  out.create = await scenario(service, root, "create", false, (d) =>
    writeFile(join(d, "b.txt"), "x"),
  );
  out.twoFiles = await scenario(service, root, "two", false, async (d) => {
    await writeFile(join(d, "x1"), "1");
    await writeFile(join(d, "x2"), "1");
  });
  out.modify = await scenario(service, root, "modify", false, (d) =>
    appendFile(join(d, "a.txt"), "x"),
  );
  out.rename = await scenario(service, root, "rename", false, (d) =>
    rename(join(d, "a.txt"), join(d, "c.txt")),
  );
  out.remove = await scenario(service, root, "remove", false, (d) => rm(join(d, "a.txt")));
  out.subWriteFlat = await scenario(service, root, "subflat", false, (d) =>
    appendFile(join(d, "sub", "n.txt"), "x"),
  );
  out.subWriteRecursive = await scenario(service, root, "subrec", true, (d) =>
    appendFile(join(d, "sub", "n.txt"), "x"),
  );
  out.mkdirRecursive = await scenario(service, root, "mkdirrec", true, (d) =>
    mkdir(join(d, "sub", "deep")),
  );
  out.removeSelfFlat = await scenario(service, root, "selfflat", false, (d) =>
    rm(d, { recursive: true }),
  );
  // 空目录自身被删：只有目录自身事件，才能暴露「根自身」的路径映射（非递归 / 递归）。
  out.removeEmptySelfFlat = await scenario(
    service,
    root,
    "emptyflat",
    false,
    (d) => rm(d, { recursive: true }),
    false,
  );
  out.removeEmptySelfRecursive = await scenario(
    service,
    root,
    "emptyrec",
    true,
    (d) => rm(d, { recursive: true }),
    false,
  );
  out.removeSelfRecursive = await scenario(service, root, "selfrec", true, (d) =>
    rm(d, { recursive: true }),
  );

  // unwatch 之后不再收到事件；待发事件被取消。
  const dir = join(root, "unwatched");
  await mkdir(dir, { recursive: true });
  const { id } = await service.watch({ path: dir });
  const late: unknown[] = [];
  service.onDynamicChange(id)((event) => late.push(event));
  await writeFile(join(dir, "z"), "1");
  await service.unwatch({ id });
  await sleep(SETTLE_MS);
  out.afterUnwatch = late;
  out.unwatchTwice = await service.unwatch({ id }).then(() => "<undefined>");

  // 不存在的目录：错误文案；失败也消耗 id。
  const missing = join(root, "missing-dir");
  out.watchMissing = await service.watch({ path: missing }).catch((e: Error) => e.message);
  const next = await service.watch({ path: dir });
  out.nextId = Number(next.id) - Number(id);
  // stale id 的订阅不抛错。
  out.staleSubscribe = await Promise.resolve()
    .then(() => {
      service.onDynamicChange("9999")(() => {});
      return "ok";
    })
    .catch((e: Error) => e.message);
  out.disposeAll = (await service.disposeAll()) ?? "<undefined>";
  return out;
}

/** 绝对路径归一化：测试运行时把 `$FIXTURE_ROOT` 换成同一工作根。 */
function normalize(value: unknown, root: string): unknown {
  return JSON.parse(JSON.stringify(value).split(root).join("$FIXTURE_ROOT"));
}

const base = await realpath(await mkdtemp(join(tmpdir(), "rusthost-file-watcher-golden-")));
const work = join(base, "work");
await mkdir(work, { recursive: true });

try {
  const service = createFileWatcherService();
  const observation = await drive(service, work);
  service.disposeAll();

  const golden = {
    _readme:
      "rustHostFileWatcher.test.ts 的 TS createFileWatcherService 观察冻结（Phase 3 golden freeze）。绝对临时路径归一化为 $FIXTURE_ROOT；事件在场景内按到达顺序记录，watcher id 仅记录相对增量；测试对 Rust Host 输出做同一变换。",
    expected: normalize(observation, work),
  };

  const goldenDir = "packages/desktop/test/fixtures/golden";
  const jsonPath = join(goldenDir, "rustHostFileWatcher.oracle.json");
  const json = `${JSON.stringify(golden, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    deepEqual(
      JSON.parse(await readFile(jsonPath, "utf8")),
      JSON.parse(json),
      "rustHostFileWatcher golden 与当前 TS 观察漂移",
    );
    console.log("rustHostFileWatcher golden: OK");
  } else {
    await mkdir(goldenDir, { recursive: true });
    await writeFile(jsonPath, json);
    console.log("wrote", jsonPath, "root=", work);
  }
} finally {
  await rm(base, { recursive: true, force: true });
}
