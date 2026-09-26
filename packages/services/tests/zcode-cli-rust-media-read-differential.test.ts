import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import {
  v4AttachmentBeginResultSchema,
  v4AttachmentChunkResultSchema,
  v4AttachmentCommitResultSchema,
} from "@zcode/shared/zcode-protocol-v4";
import { tmpdir } from "node:os";
import { crc32, deflateSync } from "node:zlib";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { titleReply, titleRequest } from "./zcode-cli-rust-title-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { anthropic, responses } from "./zcode-cli-rust-protocol-fixture.js";

// docs/specs/rust-media-read.md：Read 图片后，Node 与 Rust 在三种协议下发给模型的工具结果与
// 媒体消息形态一致（预算内原图字节一致）。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const properties = {
  contextWindow: 200000,
  inputFormat: {
    supportsText: true,
    supportsImage: true,
    supportsPdf: false,
    supportsVideo: false,
    supportsAudio: false,
  },
  outputFormat: { supportsText: true },
};
// 4×3 RGBA PNG（真实编码，Jimp 与 Rust 都能解码）；字节远小于预算，两侧应原样透传。
function rgbaPng(width: number, height: number): Buffer {
  const chunk = (type: string, data: Buffer) => {
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 6, 0, 0, 0], 8);
  const rows = Buffer.concat(
    Array.from({ length: height }, (_, y) =>
      Buffer.from([
        0,
        ...Array.from({ length: width }, (_, x) => [x * 60, y * 80, 128, 255]).flat(),
      ]),
    ),
  );
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
const png = rgbaPng(4, 3);
// 超出 2000 边长：两侧都缩放并转 JPEG（编码器不同，只比对格式与尺寸）。
const wide = rgbaPng(2400, 100);

/** 媒体 data URL → {mime, width, height}（PNG IHDR / JPEG SOF），用于跨编码器比对。 */
function describe(url: string) {
  const [, mime, data] = /^data:([^;]+);base64,(.*)$/.exec(url) ?? [];
  const bytes = Buffer.from(data ?? "", "base64");
  if (mime === "image/png")
    return { mime, width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
  for (let i = 2; i < bytes.length; ) {
    const marker = bytes[i + 1]!;
    const length = bytes.readUInt16BE(i + 2);
    if (marker >= 0xc0 && marker <= 0xc3)
      return { mime, width: bytes.readUInt16BE(i + 7), height: bytes.readUInt16BE(i + 5) };
    i += 2 + length;
  }
  return { mime };
}

type Target = {
  name: string;
  bytes: Buffer;
  args?: Record<string, unknown>;
  props?: Record<string, unknown>;
};
async function observe(kind: "node" | "rust", apiType: string, image: Buffer | Target = png) {
  const target: Target = Buffer.isBuffer(image) ? { name: "pic.png", bytes: image } : image;
  const root = await mkdtemp(join(tmpdir(), `zcode-media-${kind}-`));
  const requests: any[] = [];
  let step = 0;
  const respond = (req: any, res: any) => {
    // 标题 sidecar 请求不入用例的 requests 记录：Node 非流式、Rust 流式，两种形态都要应答。
    if (titleRequest(req)) return titleReply(res, req);
    requests.push(req);
    const call =
      step++ === 0
        ? {
            name: "Read",
            input: { file_path: join(root, "workspace", target.name), ...target.args },
          }
        : false;
    if (apiType === "openai-responses") return responses(res, call);
    if (apiType === "anthropic-messages") return anthropic(res, call);
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (call) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "call-1",
            type: "function",
            function: { name: call.name, arguments: JSON.stringify(call.input) },
          },
        ],
      });
      end(res, "tool_calls");
    } else {
      event(res, { content: "seen" });
      end(res, "stop");
    }
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo" });
  // 原因：观察过程失败时，finally 中 f.close() 会因子进程未退出被 SIGKILL 而抛出，掩盖真正的失败（CI 36214138039）。
  // 依据：先记录原始错误，关闭失败只在没有原始错误时上抛。
  let failure: unknown;
  try {
    await configureRegistry(f, false, { apiType, properties: target.props ?? properties });
    await writeFile(join(f.cwd, target.name), target.bytes);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "look at pic.png" }));
    await h.completed(id);
    const second = requests[1];
    const body = second?.messages ?? second?.input ?? [];
    // 只比对 assistant 工具调用之后的部分（工具结果与媒体消息）。
    const index = body.findIndex(
      (m: any) =>
        m.tool_calls ||
        m.type === "function_call" ||
        (Array.isArray(m.content) && m.content.some((p: any) => p.type === "tool_use")),
    );
    // JSON 文本中 Windows 路径的反斜杠已转义，按转义后的形式替换。
    const escaped = JSON.stringify(f.cwd).slice(1, -1);
    const tail = JSON.parse(JSON.stringify(body.slice(index + 1)).replaceAll(escaped, "<cwd>"));
    const read = (requests[0]?.tools ?? []).find(
      (t: any) => (t.function?.name ?? t.name) === "Read",
    );
    const observation = { tail, read, schemaErrors: h.schemaErrors };
    await h.close();
    return observation;
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    await f.close().catch((error) => {
      if (failure === undefined) throw error;
    });
  }
}

test("Node and Rust resize an oversized Read image to the same format and dimensions", async () => {
  const shape = (tail: any[]) =>
    tail.map((m) =>
      Array.isArray(m.content)
        ? {
            ...m,
            content: m.content.map((p: any) => (p.image_url ? describe(p.image_url.url) : p)),
          }
        : m,
    );
  const node = await observe("node", "openai-chat-completions", wide);
  const rust = await observe("rust", "openai-chat-completions", wide);
  assert.deepEqual(shape(node.tail)[1].content[1], { mime: "image/jpeg", width: 2000, height: 83 });
  assert.deepEqual(shape(rust.tail), shape(node.tail));
});

for (const apiType of ["openai-chat-completions", "anthropic-messages", "openai-responses"]) {
  test(`Node and Rust project Read image results the same way (${apiType})`, async () => {
    const node = await observe("node", apiType);
    assert.deepEqual(node.schemaErrors, []);
    const rust = await observe("rust", apiType);
    assert.deepEqual(rust.tail, node.tail);
    assert.deepEqual(rust.schemaErrors, []);
  });
}

// 第 2 期：模型支持 PDF 时 Read 的 schema/描述随之变化；原生 PDF 与 pages 渲染（Poppler 缺失时两侧给出相同失败）。
const pdfProps = {
  ...properties,
  inputFormat: { ...properties.inputFormat, supportsPdf: true },
};
const pdf = Buffer.from(
  [
    "%PDF-1.4",
    "1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj",
    "2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj",
    "3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 72 72]>>endobj",
    "trailer<</Root 1 0 R>>",
    "%%EOF",
    "",
  ].join("\n"),
);
for (const apiType of ["openai-chat-completions", "anthropic-messages", "openai-responses"]) {
  for (const pages of [undefined, "1"]) {
    test(`Node and Rust read PDFs the same way (${apiType}, pages=${pages ?? "none"})`, async () => {
      const target = {
        name: "doc.pdf",
        bytes: pdf,
        props: pdfProps,
        ...(pages ? { args: { pages } } : {}),
      };
      const node = await observe("node", apiType, target);
      const rust = await observe("rust", apiType, target);
      assert.deepEqual(rust.read, node.read);
      assert.deepEqual(rust.tail, node.tail);
      assert.deepEqual(rust.schemaErrors, []);
    });
  }
}

// 第 3 期：视频 Read 直传；所有协议都文本化并后置 user part（Responses 无视频输入，不覆盖）。
const videoProps = {
  ...properties,
  inputFormat: { ...properties.inputFormat, supportsVideo: true },
};
const clip = Buffer.from([0, 0, 0, 20, 102, 116, 121, 112, 105, 115, 111, 109, 0, 0, 2, 0]);
for (const apiType of ["openai-chat-completions", "anthropic-messages"]) {
  test(`Node and Rust read videos the same way (${apiType})`, async () => {
    const target = { name: "clip.mp4", bytes: clip, props: videoProps };
    const node = await observe("node", apiType, target);
    const rust = await observe("rust", apiType, target);
    assert.deepEqual(rust.tail, node.tail);
    assert.deepEqual(rust.schemaErrors, []);
  });
}

// 第 4 期：Composer 图片附件与 Read 图片走同一预算（TS prepareImageDataUrl → prepareForModel，
// 2000 边长 / 3.75MB 原始字节）；无法解码时以 `[Attached <mime>: <path>]` 占位，不中断请求。
async function observeAttachment(kind: "node" | "rust", name: string, bytes: Buffer) {
  const root = await mkdtemp(join(tmpdir(), `zcode-att-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "seen" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo" });
  try {
    await configureRegistry(f, false, { apiType: "openai-chat-completions", properties });
    const file = join(f.cwd, name);
    await writeFile(file, bytes);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(
      h.envelope("sendText", id, {
        text: "describe",
        attachments: [{ ref: file, fileName: name, mime: "image/png", bytes: bytes.length }],
      }),
    );
    await h.completed(id);
    const user = (requests[0]?.messages ?? []).findLast((m: any) => m.role === "user");
    const escaped = JSON.stringify(f.cwd).slice(1, -1);
    const content = Array.isArray(user?.content)
      ? user.content.map((p: any) =>
          p.image_url
            ? describe(p.image_url.url)
            : JSON.parse(JSON.stringify(p).replaceAll(escaped, "<cwd>")),
        )
      : user?.content;
    const observation = { content, schemaErrors: h.schemaErrors };
    await h.close();
    return observation;
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    await f.close().catch((error) => {
      if (failure === undefined) throw error;
    });
  }
}

for (const [label, name, bytes] of [
  ["within budget", "small.png", png],
  ["oversized", "wide.png", wide],
  ["undecodable", "broken.png", Buffer.from("not really a png")],
] as const) {
  test(`Node and Rust prepare ${label} image attachments the same way`, async () => {
    const node = await observeAttachment("node", name, bytes);
    const rust = await observeAttachment("rust", name, bytes);
    assert.deepEqual(node.schemaErrors, []);
    if (label === "oversized")
      assert.ok(
        JSON.stringify(node.content).includes('"width":2000'),
        JSON.stringify(node.content),
      );
    assert.deepEqual(rust.content, node.content);
    assert.deepEqual(rust.schemaErrors, []);
  });
}

// 上传的图片附件：TS ensureMediaAttachmentPath 把原始字节派生到
// `<storageRoot>/cli/image-cache/<session>/image-<sha256(ref)[..32]>.<ext>`，并在用户消息末尾告知模型该路径。
async function observeUpload(kind: "node" | "rust", bytes: Buffer) {
  const root = await mkdtemp(join(tmpdir(), `zcode-upload-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "seen" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo" });
  try {
    await configureRegistry(f, false, { apiType: "openai-chat-completions", properties });
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    const p = {
      connectionId: "fixture-desktop",
      sessionId: id,
      uploadId: "upload-1",
      fileName: "pasted.png",
      mime: "image/png",
      totalBytes: bytes.length,
      totalChunks: 1,
      checksum: `sha256:${createHash("sha256").update(bytes).digest("hex")}`,
    };
    const terminal = { connectionId: p.connectionId, sessionId: id, uploadId: p.uploadId };
    await h.client.request("v4/attachment/begin", p, v4AttachmentBeginResultSchema);
    await h.client.request(
      "v4/attachment/chunk",
      { ...terminal, chunkIndex: 0, dataBase64: bytes.toString("base64") },
      v4AttachmentChunkResultSchema,
    );
    const { ref } = await h.client.request(
      "v4/attachment/commit",
      terminal,
      v4AttachmentCommitResultSchema,
    );
    await h.command(
      h.envelope("sendText", id, {
        text: "describe",
        attachments: [{ ref, fileName: "pasted.png", mime: "image/png", bytes: bytes.length }],
      }),
    );
    await h.completed(id);
    const user = (requests[0]?.messages ?? []).findLast((m: any) => m.role === "user");
    const segment = id.replace(/[^a-zA-Z0-9._-]/g, "_").slice(0, 120);
    const derived = join(
      root,
      ".zcode",
      "cli",
      "image-cache",
      segment,
      `image-${createHash("sha256").update(ref).digest("hex").slice(0, 32)}.png`,
    );
    const content = (Array.isArray(user?.content) ? user.content : [user?.content]).map((p: any) =>
      p?.image_url
        ? describe(p.image_url.url)
        : p?.text === `[Image: source: ${derived}]`
          ? { type: "text", text: "[Image: source: <derived>]" }
          : p,
    );
    const stored = await readFile(derived).catch(() => null);
    const observation = {
      content,
      derivedMatchesUpload: stored?.equals(bytes) ?? false,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    await f.close().catch((error) => {
      if (failure === undefined) throw error;
    });
  }
}

test("Node and Rust derive the same local path for uploaded image attachments", async () => {
  const node = await observeUpload("node", wide);
  const rust = await observeUpload("rust", wide);
  assert.equal(node.derivedMatchesUpload, true, JSON.stringify(node.content));
  assert.deepEqual(rust, node);
});
