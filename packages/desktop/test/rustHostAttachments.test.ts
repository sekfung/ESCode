import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, realpath, rm, truncate, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  createFileService,
  createLocalPromptAttachmentTransferService,
  createMediaPreviewService,
} from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-attachments.md §4：prompt-attachment-transfer（本地）与 media-preview（内联）对真实 TS 实现逐项比较。

async function outcome<T>(run: () => Promise<T>) {
  try {
    return { ok: true as const, value: JSON.parse(JSON.stringify((await run()) ?? null)) };
  } catch (error) {
    return { ok: false as const, message: (error as Error).message };
  }
}

describe("rust host attachments", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the TS local attachment transfer and inline media preview",
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

      const tsTransfer = createLocalPromptAttachmentTransferService();
      const tsPreview = createMediaPreviewService({ fileService: createFileService() });
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir: base });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));

        const stage = (service: typeof tsTransfer, params: Record<string, unknown>) =>
          outcome(() => service.stage(params as never));
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
        for (const params of cases) {
          expect({
            params,
            result: await stage(services.promptAttachmentTransferService, params),
          }).toEqual({
            params,
            result: await stage(tsTransfer, params),
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

        const prepare = (service: typeof tsPreview, path: string, kind: "audio" | "video") =>
          outcome(() => service.prepare({ path, expectedKind: kind }));
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
        for (const [path, kind] of previews) {
          const actual = await prepare(services.mediaPreviewService, path, kind);
          const expected = await prepare(tsPreview, path, kind);
          if (!expected.ok) {
            expect({ path, kind, ok: actual.ok }).toEqual({ path, kind, ok: false });
            // 缺失文件的 Node 错误文案带 errno 细节，由 file.stat 差分守住；其余文案逐字相同。
            if (!path.endsWith("gone.mp3"))
              expect({ path, ...actual }).toEqual({ path, ...expected });
          } else {
            expect({ path, kind, result: actual }).toEqual({ path, kind, result: expected });
          }
        }
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
