import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { appendFile, mkdir, mkdtemp, realpath, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createFileWatcherService } from "@escode/services/node";
import type { IFileWatcherService } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-file-watcher.md §5.1 TS oracle 差分：同一脚本分别驱动真实 TS `createFileWatcherService()`
// （node:fs watch）与 Rust Host 的 `file-watcher` channel（真实 `@escode/client`），比较每个场景收到的事件序列。

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** 防抖 150 ms + 余量。 */
const SETTLE_MS = 700;

type Act = (dir: string) => Promise<void>;

async function scenario(
  service: IFileWatcherService,
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

async function drive(service: IFileWatcherService, root: string): Promise<Record<string, unknown>> {
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

describe("rust host file-watcher domain", () => {
  it.skipIf(!existsSync(hostBinary) || process.platform !== "linux")(
    "matches the TS file watcher service event for event",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-fw-")));
      const home = join(base, "home");
      const host = startHost(home);
      try {
        const rootTs = join(base, "ts");
        const rootRust = join(base, "rust");
        await mkdir(rootTs, { recursive: true });
        await mkdir(rootRust, { recursive: true });
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));

        const expected = JSON.parse(
          JSON.stringify(await drive(createFileWatcherService(), rootTs))
            .split(rootTs)
            .join("<ROOT>"),
        );
        const actual = JSON.parse(
          JSON.stringify(
            await drive(services.fileWatcherService as unknown as IFileWatcherService, rootRust),
          )
            .split(rootRust)
            .join("<ROOT>"),
        );
        expect(actual).toEqual(expected);
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    180_000,
  );
});
