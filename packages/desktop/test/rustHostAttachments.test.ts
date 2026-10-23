import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, realpath, rm, truncate, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// Phase 3 golden freeze：TS `createMediaPreviewService` / `createLocalPromptAttachmentTransferService`
// 的观察已冻结为仓内资产（packages/desktop/test/fixtures/golden/）。测试不再实例化 TS 服务，
// 删除 TS 实现后仍能独立重验。
const goldenFile = new URL("./fixtures/golden/rustHostAttachments.oracle.json", import.meta.url);

type Outcome = { ok: true; value: unknown } | { ok: false; message?: string };

interface StageCase {
  params: Record<string, unknown>;
  result: Outcome;
}

interface PreviewCase {
  path: string;
  expectedKind: "audio" | "video";
  result: Outcome;
}

interface AttachmentsGolden {
  transfer: { stageCases: StageCase[] };
  previews: { inline: PreviewCase[]; localUrl: PreviewCase[] };
}

/** 冻结资产对 >1 MiB 的 base64 只保留 sha256；两侧同法归一后再比较。 */
const LARGE_STRING = 1024 * 1024;
function sealLargeStrings<T>(value: T): T {
  if (typeof value === "string") {
    return (
      value.length > LARGE_STRING
        ? `sha256:${createHash("sha256").update(value).digest("hex")}`
        : value
    ) as T;
  }
  if (Array.isArray(value)) return value.map((item) => sealLargeStrings(item)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, sealLargeStrings(item)]),
    ) as T;
  }
  return value;
}

async function outcome<T>(run: () => Promise<T>) {
  try {
    const value = JSON.parse(JSON.stringify((await run()) ?? null));
    return { ok: true as const, value: sealLargeStrings(value) };
  } catch (error) {
    return { ok: false as const, message: (error as Error).message };
  }
}

/** 把冻结资产里的 `$FIXTURE_ROOT` / URL 编码形式替换为当前测试的临时根目录。 */
function substitute<T>(value: T, root: string): T {
  if (typeof value === "string") {
    return value
      .replaceAll("$FIXTURE_ROOT_ENCODED", encodeURIComponent(root))
      .replaceAll("$FIXTURE_ROOT", root) as T;
  }
  if (Array.isArray(value)) return value.map((item) => substitute(item, root)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, substitute(item, root)]),
    ) as T;
  }
  return value;
}

async function loadGolden(base: string): Promise<AttachmentsGolden> {
  const committed = JSON.parse(await readFile(goldenFile, "utf8")) as AttachmentsGolden;
  return substitute(committed, base);
}

// docs/specs/rust-u2-attachments.md §4：prompt-attachment-transfer（本地）与 media-preview（内联）
// 对冻结的 TS 观察逐项比较。
describe("rust host attachments", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the frozen TS local attachment transfer and inline media preview",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-att-")));
      const home = join(base, "home");
      const files = join(base, "files");
      await mkdir(files, { recursive: true });
      const small = join(files, "song.mp3");
      await writeFile(small, "abcdefgh");
      const upper = join(files, "CLIP.MP4");
      await writeFile(upper, Buffer.alloc(1024, 7));
      const text = join(files, "notes.txt");
      await writeFile(text, "x");
      const folder = join(files, "folder.webm");
      await mkdir(folder);
      const exact = join(files, "exact.wav");
      await writeFile(exact, "");
      await truncate(exact, 8 * 1024 * 1024);
      const over = join(files, "over.ogg");
      await writeFile(over, "");
      await truncate(over, 8 * 1024 * 1024 + 1);

      const golden = await loadGolden(base);
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir: base });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));

        const stage = (params: Record<string, unknown>) =>
          outcome(() => services.promptAttachmentTransferService.stage(params as never));
        const base_ = {
          operationId: "op",
          sessionId: "s",
          workspacePath: files,
          fileName: "f",
          mime: "m",
        };
        const cases: Array<Record<string, unknown>> = [
          { ...base_, localPath: small, sizeBytes: 123 },
          { ...base_, localPath: small },
          { ...base_, localPath: small, sizeBytes: 0 },
          { ...base_, localPath: join(files, "missing.bin") },
          { ...base_, localPath: small, sizeBytes: -5 },
        ];
        expect(cases).toHaveLength(golden.transfer.stageCases.length);
        for (const [index, params] of cases.entries()) {
          const expected = golden.transfer.stageCases[index];
          expect({ params, result: await stage(params) }).toEqual({
            params: expected.params,
            result: expected.result,
          });
        }
        for (const method of ["adopt", "cancel", "cleanup"] as const) {
          expect(await services.promptAttachmentTransferService[method]("op")).toBeUndefined();
        }
        const subscription = services.promptAttachmentTransferService.onDynamicProgress("op")(
          () => {
            throw new Error("本地实现不应发进度");
          },
        );
        subscription.dispose();

        const prepare = (path: string, kind: "audio" | "video") =>
          outcome(() => services.mediaPreviewService.prepare({ path, expectedKind: kind }));
        const previews: Array<[string, "audio" | "video"]> = [
          [small, "audio"],
          [small, "video"],
          [upper, "video"],
          [text, "audio"],
          [folder, "video"],
          [join(files, "gone.mp3"), "audio"],
          [exact, "audio"],
          [over, "audio"],
        ];
        expect(previews).toHaveLength(golden.previews.inline.length);
        for (const [index, [path, kind]] of previews.entries()) {
          const expected = golden.previews.inline[index];
          expect({ path, expectedKind: kind }).toEqual({
            path: expected.path,
            expectedKind: expected.expectedKind,
          });
          const actual = await prepare(path, kind);
          if (!expected.result.ok) {
            expect({ path, kind, ok: actual.ok }).toEqual({ path, kind, ok: false });
            // 缺失文件的 Node 错误文案带 errno 细节；其余文案逐字比较（与旧 TS 差分一致）。
            if (expected.result.message !== undefined) {
              expect({ path, ...actual }).toEqual({ path, ...expected.result });
            }
          } else {
            expect({ path, kind, result: actual }).toEqual({ path, kind, result: expected.result });
          }
        }
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );

  it.skipIf(!existsSync(hostBinary))(
    "returns local-url like the frozen TS observation when Main authorizes (docs §5)",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-lurl-")));
      const home = join(base, "home");
      const files = join(base, "files");
      await mkdir(files, { recursive: true });
      const spaced = join(files, "我的 song#1.mp3");
      await writeFile(spaced, "abcdefgh");
      const linked = join(files, "link-CLIP.MP4");
      await writeFile(linked, Buffer.alloc(1024, 7));
      const big = join(files, "big.mp4");
      await writeFile(big, "");
      await truncate(big, 8 * 1024 * 1024 + 1);
      const denied = join(files, "denied.mp3");
      await writeFile(denied, "x");
      const text = join(files, "notes.txt");
      await writeFile(text, "x");

      // 桩 Main 的授权：规范化（去掉 link- 前缀）或拒绝；冻结观察即由同一语义生成。
      const authorize = async (path: string) => {
        if (path.includes("denied")) throw new Error("path not authorized");
        return path.replace("link-", "");
      };
      const golden = await loadGolden(base);
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({
          kind: "rpcListen",
          bind: "127.0.0.1:0",
          home,
          dataBaseDir: base,
          localMediaPreview: true,
        });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const requests: string[] = [];
        void (async () => {
          for (;;) {
            const message = await host.next();
            if (message.kind !== "localMediaPreviewAuthorize") continue;
            const path = String(message.path);
            requests.push(path);
            try {
              host.send({
                kind: "localMediaPreviewAuthorized",
                requestId: message.requestId,
                ok: true,
                path: await authorize(path),
              });
            } catch (error) {
              host.send({
                kind: "localMediaPreviewAuthorized",
                requestId: message.requestId,
                ok: false,
                error: (error as Error).message,
              });
            }
          }
        })();

        const cases: Array<[string, "audio" | "video"]> = [
          [spaced, "audio"],
          [linked, "video"],
          [big, "video"],
          [denied, "audio"],
          [text, "audio"],
          [spaced, "video"],
        ];
        expect(cases).toHaveLength(golden.previews.localUrl.length);
        for (const [index, [path, kind]] of cases.entries()) {
          const expected = golden.previews.localUrl[index];
          expect({ path, expectedKind: kind }).toEqual({
            path: expected.path,
            expectedKind: expected.expectedKind,
          });
          const actual = await outcome(() =>
            services.mediaPreviewService.prepare({ path, expectedKind: kind }),
          );
          expect({ path, kind, result: actual }).toEqual({ path, kind, result: expected.result });
        }
        // 校验先于授权：格式不符/非文件的路径不应打扰 Main。
        expect(requests).not.toContain(text);
        expect(requests.filter((path) => path === spaced)).toHaveLength(1);
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
