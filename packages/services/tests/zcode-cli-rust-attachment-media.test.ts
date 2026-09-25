import assert from "node:assert/strict";
import test from "node:test";
import { writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { crc32, deflateSync } from "node:zlib";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { responses, anthropic } from "./zcode-cli-rust-protocol-fixture.js";

const properties = {
  inputFormat: {
    supportsText: true,
    supportsImage: true,
    supportsPdf: true,
    supportsVideo: true,
    supportsAudio: false,
  },
  outputFormat: { supportsText: true },
};
// 1×1 灰度+alpha PNG。修复：原 fixture 的 IDAT CRC 错误，Jimp 与 Rust 都无法解码；附件图片现在按 TS 预算
// 解码/压缩（docs/specs/rust-media-read.md 第 4 期），坏图会变成占位文本，因此换成合法 PNG。
function grayPng(): Buffer {
  const chunk = (type: string, data: Buffer) => {
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(1, 0);
  header.writeUInt32BE(1, 4);
  header.set([8, 4, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(Buffer.from([0, 0xff, 0xff]))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
const png = grayPng();
for (const apiType of ["openai-chat-completions", "anthropic-messages"]) {
  test(`Rust ${apiType} preserves the TS gateway video content shape`, async () => {
    const f = await fixture({
      config: { apiType, reasoningParameters: {}, formatProperties: properties },
      respond(_req, res) {
        if (apiType === "anthropic-messages") anthropic(res);
        else {
          res.writeHead(200, { "content-type": "text/event-stream" });
          event(res, { content: "video accepted" });
          end(res, "stop");
        }
      },
    });
    try {
      const h = f.start();
      const id = await h.create();
      await h.subscribe(`conversation/${id}`);
      const path = join(f.cwd, "clip.mp4");
      const bytes = Buffer.from([0, 0, 0, 20, 102, 116, 121, 112, 105, 115, 111, 109]);
      await writeFile(path, bytes);
      await h.command(
        h.envelope("sendText", id, {
          text: "video",
          attachments: [
            { ref: path, fileName: "clip.mp4", mime: "video/mp4", bytes: bytes.length },
          ],
        }),
      );
      await h.completed(id);
      const content = f.requests[0]!.messages.findLast((m: any) => m.role === "user").content;
      // Anthropic 合并相邻 user 消息，context reminder 位于实际附件文本之前。
      if (apiType === "anthropic-messages") assert.match(content[0].text, /^<system-reminder>/);
      const video = content[apiType === "anthropic-messages" ? 2 : 1];
      if (apiType === "anthropic-messages")
        assert.deepEqual(video, {
          type: "video",
          source: { type: "base64", media_type: "video/mp4", data: bytes.toString("base64") },
        });
      else
        assert.deepEqual(video, {
          type: "video_url",
          video_url: { url: `data:video/mp4;base64,${bytes.toString("base64")}` },
        });
      // TS projectMessagesWithMediaAttachmentPaths：本地媒体在消息末尾附来源路径；它是最新消息的最后一块，
      // TS finalizeLatestNonSystemMessageCacheControl 的缓存断点因此落在这段文本上。
      assert.deepEqual(content.at(-1), {
        type: "text",
        text: `[Video: source: ${path}]`,
        ...(apiType === "anthropic-messages" ? { cache_control: { type: "ephemeral" } } : {}),
      });
      assert.deepEqual(h.schemaErrors, []);
    } finally {
      await f.close();
    }
  });
}

test("Rust transports media above the text request budget and reports a missing cold snapshot without an HTTP request", async () => {
  const f = await fixture({ config: { formatProperties: properties } });
  try {
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    const path = join(f.cwd, "large.pdf");
    const bytes = Buffer.from(`%PDF-1.4\n${"a".repeat(2 * 1024 * 1024)}\n%%EOF`);
    await writeFile(path, bytes);
    await h.command(
      h.envelope("sendText", id, {
        text: "read",
        attachments: [
          { ref: path, fileName: "large.pdf", mime: "application/pdf", bytes: bytes.length },
        ],
      }),
    );
    await h.completed(id);
    assert(f.requestBodies[0]!.length > 2 * 1024 * 1024);
    await h.close();
    const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"), { readOnly: true });
    const session = JSON.parse(
      db.prepare("SELECT body FROM rust_session WHERE id=?").get(id)!.body as string,
    );
    db.close();
    for (const asset of Object.values(session.attachments) as { path: string }[])
      await rm(asset.path);
    const restarted = f.start();
    await restarted.subscribe(`conversation/${id}`);
    await restarted.command(restarted.envelope("sendText", id, { text: "continue" }));
    const failed = await restarted.wait((m) =>
      m.params?.frame?.payload?.deltas?.some(
        (d: any) => d.patch?.control?.lastError?.code === "attachment_unavailable",
      ),
    );
    assert(failed);
    assert.equal(f.requests.length, 1);
    assert.deepEqual(restarted.schemaErrors, []);
  } finally {
    await f.close();
  }
});

for (const apiType of ["openai-chat-completions", "openai-responses", "anthropic-messages"]) {
  test(`Rust ${apiType} sends immutable image/PDF attachments as native media, keeps base64 out of storage and reuses request bytes on retry`, async () => {
    const f = await fixture({
      config: {
        apiType,
        reasoningParameters: {},
        formatProperties: properties,
        retry: { maxRetries: 1, baseDelayMs: 1, maxDelayMs: 1, jitter: false },
      },
      respond(_req, res, attempt) {
        if (attempt === 1) {
          res.writeHead(429);
          res.end();
          return;
        }
        if (apiType === "openai-responses") responses(res);
        else if (apiType === "anthropic-messages") anthropic(res);
        else {
          res.writeHead(200, { "content-type": "text/event-stream" });
          event(res, { content: "saw attachment" });
          end(res, "stop");
        }
      },
    });
    try {
      const h = f.start();
      const id = await h.create();
      await h.subscribe(`conversation/${id}`);
      const path = join(f.cwd, "image.png");
      const pdf = join(f.cwd, "document.pdf");
      await writeFile(path, png);
      await writeFile(pdf, "%PDF-1.4\nfixture\n%%EOF");
      const ack = await h.command(
        h.envelope("sendText", id, {
          text: "describe",
          attachments: [
            { ref: path, fileName: "image.png", mime: "image/png", bytes: png.length },
            { ref: pdf, fileName: "document.pdf", mime: "application/pdf", bytes: 23 },
          ],
        }),
      );
      assert.equal(ack.status, "accepted");
      await h.completed(id);
      assert.equal(f.requests.length, 2);
      assert.equal(f.requestBodies[0], f.requestBodies[1]);
      const body = f.requests[0]!;
      let content = (apiType === "openai-responses" ? body.input : body.messages).findLast(
        (m: any) => m.role === "user",
      ).content;
      if (apiType === "anthropic-messages") {
        assert.equal(content[0].type, "text");
        assert.match(content[0].text, /^<system-reminder>/);
        content = content.slice(1);
      }
      assert.deepEqual(
        content.map((p: any) => p.type),
        apiType === "openai-responses"
          ? ["input_text", "input_image", "input_file", "input_text", "input_text"]
          : apiType === "anthropic-messages"
            ? ["text", "image", "document", "text", "text"]
            : ["text", "image_url", "file", "text", "text"],
      );
      assert.deepEqual(
        content.slice(-2).map((p: any) => p.text),
        [`[Image: source: ${path}]`, `[PDF: source: ${pdf}]`],
      );
      assert.match(
        JSON.stringify(content),
        new RegExp(png.toString("base64").replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
      );
      assert(!f.requestBodies[0]!.includes(f.dataDir));
      assert(!f.requestBodies[0]!.includes("_zcode_attachment"));
      const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"), { readOnly: true });
      const stored = db
        .prepare("SELECT body FROM rust_message WHERE session=?")
        .all(id)
        .map((r) => r.body)
        .join("");
      db.close();
      assert(!stored.includes(png.toString("base64")));
      assert(stored.includes("_zcode_attachment"));
      assert.deepEqual(h.schemaErrors, []);
    } finally {
      await f.close();
    }
  });
}

test("Rust rejects unsupported media and invalid PDF before committing a user turn or requesting a model", async () => {
  for (const supportsPdf of [false, true]) {
    const f = await fixture({
      config: {
        formatProperties: {
          ...properties,
          inputFormat: { ...properties.inputFormat, supportsPdf },
        },
      },
    });
    try {
      const h = f.start();
      const id = await h.create();
      const path = join(f.cwd, "bad.pdf");
      await writeFile(path, "not a PDF");
      await assert.rejects(
        h.command(
          h.envelope("sendText", id, {
            text: "read",
            attachments: [{ ref: path, fileName: "bad.pdf", mime: "application/pdf", bytes: 9 }],
          }),
        ),
        supportsPdf ? /PDF is invalid/ : /unsupported by selected model/,
      );
      assert.equal((await h.rows(id)).rows.length, 0);
      assert.equal(f.requests.length, 0);
    } finally {
      await f.close();
    }
  }
});
