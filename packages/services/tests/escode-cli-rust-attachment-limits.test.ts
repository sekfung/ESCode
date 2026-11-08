import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fixture, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, goldenOracle } from "./escode-cli-rust-golden.js";

const pluginHostBundle = resolve("apps/escode-cli/packages/cli/dist/plugin-host.cjs");
const rustPluginEnv = { ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(pluginHostBundle) };
process.env.ESCODE_TEST_REQUEST_MS ??= "60000";
// 21–30 MiB 附件的落盘与请求体读写会超过 fixture 默认 8s 的等待。
process.env.ESCODE_TEST_WAIT_MS ??= "30000";

type Runtime = "node" | "rust";
const MiB = 1024 * 1024;

const sha256 = (value: string | Buffer) => createHash("sha256").update(value).digest("hex");

/** 21 MiB 文本：超过 READ_MAX_FILE_SIZE_BYTES 后 TS 只读前 2000 行，再按 token 预算给部分视图。 */
function oversizedText() {
  return Array.from({ length: 5_200 }, (_, i) => `line ${i} ${"x".repeat(4_088)}`).join("\n");
}

/**
 * 把 data URL 正文替换成摘要，避免把 21 MiB 视频的 base64 冻进 golden；
 * mime/参数保留，body 只比较内容摘要。
 */
function scrub(value: unknown, normalize: (text: string) => string): unknown {
  if (typeof value === "string") {
    const dataUrl = /^data:([^;]*)((?:;[^,]*)?),(.*)$/s.exec(value);
    if (dataUrl) return `data:${dataUrl[1]}${dataUrl[2]},<sha256:${sha256(dataUrl[3]!)}>`;
    return normalize(value);
  }
  if (Array.isArray(value)) return value.map((item) => scrub(item, normalize));
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => (a < b ? -1 : 1))
        .map(([key, item]) => [key, scrub(item, normalize)]),
    );
  return value;
}

/** 附件进入模型请求的 user 侧消息（排除技能/日期等与附件无关的 reminder）。 */
function observeMessages(messages: Record<string, any>[], cwd: string) {
  const normalize = (text: string) =>
    text.split(JSON.stringify(cwd).slice(1, -1)).join("<CWD>").split(cwd).join("<CWD>");
  const texts = messages.map((message) =>
    typeof message.content === "string"
      ? normalize(message.content)
      : JSON.stringify(scrub(message.content, normalize)),
  );
  const attachmentMessages = messages
    .map((message, index) => ({ role: message.role, text: texts[index]! }))
    .filter(
      (message) =>
        message.role === "user" &&
        !message.text.includes("skills are available") &&
        !message.text.includes("# currentDate"),
    )
    .map((message) => message.text);
  return {
    attachmentDigest: sha256(JSON.stringify(attachmentMessages)),
    attachmentMessages: attachmentMessages.map((text) =>
      text.length > 400 ? `${text.slice(0, 200)}…${text.slice(-200)}` : text,
    ),
  };
}

async function observeAttachment(
  kind: Runtime,
  fileName: string,
  mime: string,
  body: Buffer,
  properties?: Record<string, unknown>,
) {
  const root = await mkdtemp(join(tmpdir(), `escode-diff-att-limits-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [pluginHostBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ root, env: rustPluginEnv, registry: true });
  try {
    await configureRegistry(f, false, properties ? { properties } : {});
    const file = join(f.cwd, fileName);
    await writeFile(file, body);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(
      h.envelope("sendText", id, {
        text: "describe it",
        mode: "yolo",
        attachments: [{ ref: file, fileName, mime, bytes: body.length }],
      }),
    );
    await h.completed(id);
    const observation = observeMessages(f.requests.at(-1)!.messages, f.cwd);
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

const videoProperties = {
  contextWindow: 200_000,
  inputFormat: {
    supportsText: true,
    supportsImage: true,
    supportsPdf: true,
    supportsVideo: true,
    supportsAudio: false,
  },
  outputFormat: { supportsText: true },
};

const cases: {
  name: string;
  fileName: string;
  mime: string;
  body: () => Buffer;
  properties?: Record<string, unknown>;
}[] = [
  {
    name: "audio-reference",
    fileName: "note.mp3",
    mime: "audio/mpeg",
    body: () => Buffer.from("ID3\u0004audio payload", "utf8"),
  },
  {
    name: "many-short-lines-under-256kib",
    fileName: "many-lines.txt",
    mime: "text/plain",
    body: () => Buffer.from(Array.from({ length: 3_000 }, (_, i) => `line ${i}`).join("\n")),
  },
  {
    name: "oversized-text-preview",
    fileName: "huge.txt",
    mime: "text/plain",
    body: () => Buffer.from(oversizedText()),
  },
  {
    name: "oversized-image-reference",
    fileName: "huge.png",
    mime: "image/png",
    body: () => Buffer.alloc(20 * MiB + 1, 0x37),
  },
  {
    name: "oversized-pdf-reference",
    fileName: "huge.pdf",
    mime: "application/pdf",
    body: () => Buffer.alloc(20 * MiB + 1, 0x25),
  },
  {
    name: "oversized-video-reference",
    fileName: "huge.mp4",
    mime: "video/mp4",
    body: () => Buffer.alloc(30 * MiB + 1, 0x4d),
  },
  {
    name: "inline-video-within-limit",
    fileName: "clip.mp4",
    mime: "video/mp4",
    body: () => Buffer.alloc(21 * MiB, 0x42),
    properties: videoProperties,
  },
  {
    name: "utf8-bom-text",
    fileName: "bom.txt",
    mime: "text/plain",
    body: () => Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), Buffer.from("hello\r\nworld\n")]),
  },
  {
    name: "gbk-text",
    fileName: "gbk.txt",
    mime: "text/plain",
    // “中文” 的 GBK 编码；TS 与 Rust 都必须按同一编码解码后再进入 reminder。
    body: () => Buffer.from([0xd6, 0xd0, 0xce, 0xc4, 0x0a]),
  },
];

for (const item of cases) {
  test(`Node and Rust put a ${item.name} attachment into the model request the same way`, async () => {
    const node = await goldenOracle(item.name, () =>
      observeAttachment("node", item.fileName, item.mime, item.body(), item.properties),
    );
    const rust = canonical(
      await observeAttachment("rust", item.fileName, item.mime, item.body(), item.properties),
    );
    assert.deepEqual(
      rust.attachmentMessages,
      node.attachmentMessages,
      "attachment messages differ",
    );
    assert.equal(rust.attachmentDigest, node.attachmentDigest, "full attachment content differs");
  });
}

const target = (row: Record<string, any>) => ({ rowId: row.rowId, entityId: row.entityId });

async function retry(h: Harness, id: string) {
  const rows = await h.rows(id);
  // retryTurn 的 target 是最后一条 assistantText（guard.latestAssistantRetryOnly）。
  const row = rows.rows.findLast((entry) => entry.kind === "assistantText")!;
  const command = {
    ...h.envelope("retryTurn", id, { target: target(row) }),
    baseRevision: rows.atRevision,
    baseLogEpoch: rows.atLogEpoch,
  };
  const at = h.messages.length;
  await h.command(command);
  await h.completed(id, at);
}

test("Rust replays the frozen oversized text preview after the source file is deleted", async () => {
  const f = await fixture();
  try {
    const body = Buffer.from(oversizedText());
    const file = join(f.cwd, "frozen.txt");
    await writeFile(file, body);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(
      h.envelope("sendText", id, {
        text: "describe it",
        mode: "yolo",
        attachments: [
          { ref: file, fileName: "frozen.txt", mime: "text/plain", bytes: body.length },
        ],
      }),
    );
    await h.completed(id);
    const first = observeMessages(f.requests.at(-1)!.messages, f.cwd);
    assert(first.attachmentDigest);

    await rm(file);
    await retry(h, id);
    const second = observeMessages(f.requests.at(-1)!.messages, f.cwd);
    assert.equal(second.attachmentDigest, first.attachmentDigest);
    // 预览提示在展示摘要里会被截掉中段，直接检查原始请求正文。
    assert.match(JSON.stringify(f.requests.at(-1)!.messages), /Showing a partial view/);
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});
