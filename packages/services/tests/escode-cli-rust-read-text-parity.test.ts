import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { once } from "node:events";
import { canonical, goldenOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-file-tool-results.md「Read 文本大文件与 token 预算」：
// TS readTextFileForModel + formatReadTextOutput 为 oracle，Rust Read 的模型文本与结构化输出必须逐字一致。
const readModule = "../../../apps/escode-cli/packages/core/src/tool/handlers/read.js";
const readTextModule = "../../../apps/escode-cli/packages/core/src/tool/handlers/read-text.js";
const fsAdapterModule = "../../../apps/escode-cli/packages/adapters/src/fs/index.js";

async function driver() {
  const root = await mkdtemp(join(tmpdir(), "read-text-parity-"));
  const child = spawn(
    resolve(
      `apps/escode-cli-rust/target/debug/examples/tool_fixture${process.platform === "win32" ? ".exe" : ""}`,
    ),
    [root],
  );
  let errors = "";
  child.stderr.on("data", (chunk) => {
    errors += chunk;
  });
  const lines = createInterface({ input: child.stdout });
  const call = async (name: string, args: unknown, session: string): Promise<any> => {
    const next = once(lines, "line");
    child.stdin.write(JSON.stringify({ name, args, session }) + "\n");
    return JSON.parse((await next)[0]);
  };
  return {
    root,
    call,
    async close() {
      const closed = once(child, "close");
      child.stdin.end();
      assert.deepEqual(await closed, [0, null], errors);
      lines.close();
      await rm(root, { recursive: true, force: true });
    },
  };
}

type Case = {
  name: string;
  fileName: string;
  bytes: Buffer | (() => Promise<Buffer>) | string;
  args?: Record<string, unknown>;
};

const bigText = Array.from({ length: 5000 }, (_, i) => `line ${i} ${"x".repeat(100)}`).join("\n");
const longLine = "y".repeat(200_000);
const cases: Case[] = [
  { name: "trailing newline", fileName: "small.txt", bytes: "a\nb\n" },
  { name: "no trailing newline", fileName: "nonewline.txt", bytes: "a\nb" },
  { name: "crlf", fileName: "crlf.txt", bytes: "a\r\nb\r\n" },
  { name: "empty", fileName: "empty.txt", bytes: "" },
  {
    name: "utf8 bom",
    fileName: "bom.txt",
    bytes: Buffer.from([0xef, 0xbb, 0xbf, ...Buffer.from("a\nb\n")]),
  },
  {
    name: "utf16le",
    fileName: "utf16.txt",
    bytes: Buffer.from([0xff, 0xfe, ...Buffer.from("a\nb", "utf16le")]),
  },
  { name: "gbk", fileName: "gbk.txt", bytes: Buffer.from([0xc4, 0xe3, 0xba, 0xc3, 0x0a]) },
  { name: "offset zero", fileName: "range.txt", bytes: "a\nb\nc\nd\ne\n", args: { offset: 0 } },
  {
    name: "range",
    fileName: "range.txt",
    bytes: "a\nb\nc\nd\ne\n",
    args: { offset: 2, limit: 2 },
  },
  {
    name: "offset beyond eof",
    fileName: "range.txt",
    bytes: "a\nb\nc\nd\ne\n",
    args: { offset: 10 },
  },
  {
    name: "over file size limit",
    fileName: "big.txt",
    bytes: bigText,
  },
  {
    name: "range exceeds token budget",
    fileName: "big.txt",
    bytes: bigText,
    args: { offset: 1, limit: 2000 },
  },
  {
    name: "range within token budget",
    fileName: "big.txt",
    bytes: bigText,
    args: { offset: 1, limit: 50 },
  },
  { name: "single long line partial view", fileName: "long.txt", bytes: longLine },
  {
    name: "single long line range token error",
    fileName: "long.txt",
    bytes: longLine,
    args: { offset: 1, limit: 2000 },
  },
  { name: "binary content", fileName: "binary.txt", bytes: Buffer.from([0, 1, 2, 3, 255]) },
];

function scrub<T>(value: T, root: string): T {
  if (typeof value === "string") return value.split(root).join("<ROOT>") as T;
  if (Array.isArray(value)) return value.map((item) => scrub(item, root)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, scrub(item, root)]),
    ) as T;
  }
  return value;
}

async function observeRust(d: Awaited<ReturnType<typeof driver>>, root: string) {
  const observed: { name: string; model?: unknown; data?: unknown; error?: string }[] = [];
  for (const [index, item] of cases.entries()) {
    const file = join(root, item.fileName);
    const result = await d.call("Read", { file_path: file, ...item.args }, `case-${index}`);
    observed.push(
      scrub(
        {
          name: item.name,
          ...(result.error !== undefined
            ? { error: result.error }
            : { model: result.content, data: result.data }),
        },
        root,
      ),
    );
  }
  return observed;
}

test("Node and Rust read text files with the same size, token, range and encoding semantics", async () => {
  const d = await driver();
  try {
    for (const item of cases) {
      const bytes = typeof item.bytes === "function" ? await item.bytes() : item.bytes;
      await mkdir(join(d.root, "files"), { recursive: true });
      await writeFile(join(d.root, item.fileName), bytes);
    }
    const oracle = await goldenOracle("cases", async () => {
      const { NodeFileSystemAdapter } = await import(fsAdapterModule);
      const { readHandler } = await import(readModule);
      const { formatReadTextOutput } = await import(readTextModule);
      const adapter = new NodeFileSystemAdapter();
      const observed: { name: string; model?: unknown; data?: unknown; error?: string }[] = [];
      for (const item of cases) {
        const context = {
          workingDirectory: d.root,
          workspaceRoot: d.root,
          sessionId: "fixture",
          toolCallId: "fixture",
          traceId: "fixture",
          abortSignal: new AbortController().signal,
          fileSystemPort: adapter,
          readFileState: new Map(),
        };
        try {
          const output = (await readHandler(
            { file_path: join(d.root, item.fileName), ...item.args },
            context as any,
          )) as any;
          observed.push(
            scrub(
              {
                name: item.name,
                model: output?.type === "text" ? formatReadTextOutput(output) : output,
                data: output,
              },
              d.root,
            ),
          );
        } catch (error: any) {
          observed.push(scrub({ name: item.name, error: error?.message ?? String(error) }, d.root));
        }
      }
      return observed;
    });
    const rust = canonical(await observeRust(d, d.root));
    assert.deepEqual(rust, oracle, "Read text observations differ");
  } finally {
    await d.close();
  }
});

test("Node and Rust keep token partial views out of the repeat-read cache", async () => {
  const d = await driver();
  try {
    const long = join(d.root, "long.txt");
    const small = join(d.root, "small.txt");
    await writeFile(long, "y".repeat(200_000));
    await writeFile(small, "a\nb\n");
    const oracle = await goldenOracle("partial-view-dedup", async () => {
      const { NodeFileSystemAdapter } = await import(fsAdapterModule);
      const { readHandler } = await import(readModule);
      const { formatReadTextOutput } = await import(readTextModule);
      const adapter = new NodeFileSystemAdapter();
      const context = {
        workingDirectory: d.root,
        workspaceRoot: d.root,
        sessionId: "fixture",
        toolCallId: "fixture",
        traceId: "fixture",
        abortSignal: new AbortController().signal,
        fileSystemPort: adapter,
        readFileState: new Map(),
      };
      const read = async (file: string) => {
        const output = (await readHandler({ file_path: file }, context as any)) as any;
        if (output?.type === "text") return formatReadTextOutput(output);
        if (output?.type === "file_unchanged")
          return "Wasted call — file unchanged since your last Read. Refer to that earlier tool_result instead.";
        return output;
      };
      const digest = (text: string) => createHash("sha256").update(text).digest("hex");
      return scrub(
        {
          partialTwice: [digest(await read(long)), digest(await read(long))],
          smallTwice: [digest(await read(small)), digest(await read(small))],
        },
        d.root,
      );
    });
    const readRust = async (file: string, session: string) =>
      (await d.call("Read", { file_path: file }, session)) as any;
    const digest = (text: string) => createHash("sha256").update(text).digest("hex");
    const rust = canonical({
      partialTwice: [
        digest((await readRust(long, "partial")).content),
        digest((await readRust(long, "partial")).content),
      ],
      smallTwice: [
        digest((await readRust(small, "small")).content),
        digest((await readRust(small, "small")).content),
      ],
    });
    assert.deepEqual(rust, oracle, "repeat-read behavior differs");
  } finally {
    await d.close();
  }
});
