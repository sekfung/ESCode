// Phase 3 golden freeze：rustHostAttachments.test.ts 的 TS `createMediaPreviewService` /
// `createLocalPromptAttachmentTransferService` 观察录制。一次性脚本；删除 TS 实现前用于重录。
// `--check` 校验仓内 golden 与当前 TS 观察一致。绝对临时路径归一化为 `$FIXTURE_ROOT`，
// URL 中的百分号编码形式归一化为 `$FIXTURE_ROOT_ENCODED`。
import { deepEqual } from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, realpath, rm, truncate, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildLocalMediaPreviewUrl } from "../packages/shared/src/platform.ts";
import {
  createFileService,
  createLocalPromptAttachmentTransferService,
  createMediaPreviewService,
} from "../packages/services/src/node.ts";

type Outcome = { ok: true; value: unknown } | { ok: false; message: string };

async function outcome<T>(run: () => Promise<T>): Promise<Outcome> {
  try {
    return { ok: true, value: JSON.parse(JSON.stringify((await run()) ?? null)) };
  } catch (error) {
    return { ok: false, message: (error as Error).message };
  }
}

const base = await realpath(await mkdtemp(join(tmpdir(), "rusthost-attachments-golden-")));
const files = join(base, "files");
await mkdir(files, { recursive: true });

// 第一个用例（内联预览）的输入。
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

// 第二个用例（local-url）的输入。
const spaced = join(files, "我的 song#1.mp3");
await writeFile(spaced, "abcdefgh");
const linked = join(files, "link-CLIP.MP4");
await writeFile(linked, Buffer.alloc(1024, 7));
const big = join(files, "big.mp4");
await writeFile(big, "");
await truncate(big, 8 * 1024 * 1024 + 1);
const denied = join(files, "denied.mp3");
await writeFile(denied, "x");

const encodedBase = encodeURIComponent(base);
const normalizeText = (value: string) =>
  value.replaceAll(encodedBase, "$FIXTURE_ROOT_ENCODED").replaceAll(base, "$FIXTURE_ROOT");
function normalize<T>(value: T): T {
  if (typeof value === "string") return normalizeText(value) as T;
  if (Array.isArray(value)) return value.map((item) => normalize(item)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, normalize(item)]),
    ) as T;
  }
  return value;
}

// prompt-attachment-transfer（本地）：stage 的 declared size / stat 回退 / 缺失文件。
const transfer = createLocalPromptAttachmentTransferService();
const stageBase = {
  operationId: "op",
  sessionId: "s",
  workspacePath: files,
  fileName: "f",
  mime: "m",
};
const stageCases: Array<{ params: unknown; result: unknown }> = [];
for (const params of [
  { ...stageBase, localPath: small, sizeBytes: 123 },
  { ...stageBase, localPath: small },
  { ...stageBase, localPath: small, sizeBytes: 0 },
  { ...stageBase, localPath: join(files, "missing.bin") },
  { ...stageBase, localPath: small, sizeBytes: -5 },
]) {
  stageCases.push({
    params: normalize(params),
    result: normalize(await outcome(() => transfer.stage(params as never))),
  });
}

// media-preview（内联分支）：格式/kind 校验、目录、8 MiB 边界、缺失文件。
const previewService = createMediaPreviewService({ fileService: createFileService() });
const inlinePreviews: Array<{ path: unknown; expectedKind: string; result: unknown }> = [];
for (const { path, expectedKind } of [
  { path: small, expectedKind: "audio" },
  { path: small, expectedKind: "video" },
  { path: upper, expectedKind: "video" },
  { path: text, expectedKind: "audio" },
  { path: folder, expectedKind: "video" },
  { path: join(files, "gone.mp3"), expectedKind: "audio" },
  { path: exact, expectedKind: "audio" },
  { path: over, expectedKind: "audio" },
] as const) {
  const result = await outcome(() => previewService.prepare({ path, expectedKind }));
  inlinePreviews.push({
    path: normalize(path),
    expectedKind,
    // 缺失文件的 Node 错误文案带 errno 细节且不稳定；测试只比较 ok（与旧 TS oracle 差分一致）。
    result: normalize(result.ok || !path.endsWith("gone.mp3") ? result : { ok: false }),
  });
}

// media-preview（local-url）：与测试相同的 Main 授权桩。
const authorize = async (path: string) => {
  if (path.includes("denied")) throw new Error("path not authorized");
  return path.replace("link-", "");
};
const localPreviewService = createMediaPreviewService({
  fileService: createFileService(),
  authorizeLocalMediaPreviewPath: authorize,
  createLocalMediaPreviewUrl: buildLocalMediaPreviewUrl,
});
const localUrlPreviews: Array<{ path: unknown; expectedKind: string; result: unknown }> = [];
for (const { path, expectedKind } of [
  { path: spaced, expectedKind: "audio" },
  { path: linked, expectedKind: "video" },
  { path: big, expectedKind: "video" },
  { path: denied, expectedKind: "audio" },
  { path: text, expectedKind: "audio" },
  { path: spaced, expectedKind: "video" },
] as const) {
  localUrlPreviews.push({
    path: normalize(path),
    expectedKind,
    result: normalize(await outcome(() => localPreviewService.prepare({ path, expectedKind }))),
  });
}

/**
 * 8 MiB 内联预览的 base64 有 11 MB；冻结资产只保存 sha256，避免把大 payload 写进仓库。
 * 测试侧对 Rust 输出做同一 seal 后比较。
 */
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

const golden = sealLargeStrings({
  _readme:
    "rustHostAttachments.test.ts 的 TS media-preview / prompt-attachment-transfer 观察冻结（Phase 3 golden freeze）。由真实 createMediaPreviewService/createLocalPromptAttachmentTransferService 生成一次；删除 TS 后测试只加载本资产。$FIXTURE_ROOT 由测试替换为当前临时根目录，$FIXTURE_ROOT_ENCODED 为其 URL 编码形式；超过 1 MiB 的字符串以 sha256 摘要存储。",
  transfer: { stageCases },
  previews: { inline: inlinePreviews, localUrl: localUrlPreviews },
});
const goldenDir = "packages/desktop/test/fixtures/golden";
const goldenPath = join(goldenDir, "rustHostAttachments.oracle.json");
const goldenJson = `${JSON.stringify(golden, null, 2)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(goldenPath, "utf8")),
    JSON.parse(goldenJson),
    "rustHostAttachments golden 与当前 TS 观察漂移",
  );
  console.log("rustHostAttachments golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, goldenJson);
  console.log("wrote", goldenPath);
}
await rm(base, { recursive: true, force: true });
