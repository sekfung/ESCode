import assert from "node:assert/strict";
import test from "node:test";
import { copyFile, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { createSqliteSessionStore } from "../../../apps/zcode-cli/packages/adapters/src/storage/session-store.js";
import type { SessionId, MessageId, ProjectId, WorkspaceId, PartId } from "@zcode/contracts";

// 性能验收「长历史会话」：同一份由真实 TS store 写出的长会话，分别由 Node 与 Rust 打开。
// 默认跳过；ZCODE_BENCH_PROVIDER_ID 启用（在长历史上再发一轮真实请求），ZCODE_BENCH_HISTORY_TURNS 控制轮数。
const providerId = process.env.ZCODE_BENCH_PROVIDER_ID;
const turns = Number(process.env.ZCODE_BENCH_HISTORY_TURNS ?? 300);
const rounds = Number(process.env.ZCODE_BENCH_ROUNDS ?? 5);
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const rustBinary = process.env.ZCODE_BENCH_RUST_BINARY;
const session = "long-history" as SessionId;
let benchModelId = "";
const workspace = "long-history-workspace";

async function seed(path: string, modelId: string) {
  const store = createSqliteSessionStore({ dbPath: path });
  await store.createSession({
    id: session,
    projectID: "p" as ProjectId,
    workspaceID: workspace as WorkspaceId,
    directory: workspace,
    slug: "long",
    title: "long history",
    titleSource: "custom",
    version: "bench",
  });
  // 真实会话总带模型选择；Node 在缺失时拒绝发送（Session model must be provider-qualified）。
  await store.saveSessionEntry({
    id: "selection",
    sessionID: session,
    type: "runtime/model_selection",
    time: { created: 1, updated: 1 },
    data: { providerId: "personal:bench", modelId },
  });
  let clock = 1;
  for (let i = 0; i < turns; i++) {
    const user = `u${i}` as MessageId;
    const assistant = `a${i}` as MessageId;
    await store.saveMessage({
      id: user,
      sessionID: session,
      role: "user",
      time: { created: (clock += 1) },
      agent: "main",
      modelSelection: { providerId: "personal:bench", modelId },
    });
    await store.savePart({
      id: `${user}-t` as PartId,
      sessionID: session,
      messageID: user,
      type: "text",
      text: `Question ${i}: summarize module ${i} in one line.`,
    });
    await store.saveMessage({
      id: assistant,
      sessionID: session,
      role: "assistant",
      parentID: user,
      time: { created: (clock += 1), completed: clock },
      agent: "main",
      mode: "yolo",
      path: { cwd: workspace, root: workspace },
      cost: 0,
      tokens: { input: 1, output: 1, reasoning: 0, cache: { read: 0, write: 0 } },
    });
    await store.savePart({
      id: `${assistant}-tool` as PartId,
      sessionID: session,
      messageID: assistant,
      type: "tool",
      callID: `call-${i}`,
      tool: "Read",
      declarationIndex: 0,
      state: {
        status: "completed",
        input: { file_path: `module-${i}.ts` },
        output: `export const value${i} = ${i};\n`.repeat(8),
        title: "read",
        metadata: {},
        time: { start: clock, end: clock },
      },
    });
    await store.savePart({
      id: `${assistant}-t` as PartId,
      sessionID: session,
      messageID: assistant,
      type: "text",
      text: `Module ${i} exports a single numeric constant used by the loader.`,
    });
  }
  store.close();
}

async function providerFiles(root: string) {
  const config = JSON.parse(await readFile(join(homedir(), ".zcode", "v2", "config.json"), "utf8"));
  const p = config.provider[providerId!];
  const modelId = process.env.ZCODE_BENCH_MODEL ?? Object.keys(p.models ?? {})[0];
  const builtin = JSON.parse(await readFile(resolve("config/provider/zcode-builtin.json"), "utf8"));
  builtin.config.providerConfigRules.providerRules = [];
  await writeFile(join(root, "builtin.json"), JSON.stringify(builtin));
  await writeFile(
    join(root, "personal.json"),
    JSON.stringify({
      schemaVersion: 1,
      config: {
        providerConfigRules: {
          providerRules: [
            {
              providerId: "personal:bench",
              providerName: "Bench",
              config: {
                group: "standard-personal",
                access: { type: "api-key", apiKey: p.options.apiKey || "none" },
                api:
                  p.kind === "anthropic"
                    ? { type: "anthropic-messages", baseUrl: p.options.baseURL }
                    : { type: "openai-chat-completions", baseUrl: p.options.baseURL },
                personalModelIds: [modelId],
              },
            },
          ],
        },
        // 显式模型选择需要思考深度（Node 否则拒绝建模）；两侧同一规则。
        modelConfigRules: {
          providerModelRules: [
            {
              providerId: "personal:bench",
              modelId,
              config: {
                optionSpecs: {
                  reasoningLevel: {
                    values: ["low", "high"],
                    map: "{'reasoning_effort': reasoningLevel}",
                  },
                },
              },
            },
          ],
          manualProviderModelRules: [],
        },
        defaultModelSelection: {
          providerId: "personal:bench",
          modelId,
          options: { reasoningLevel: "high" },
        },
      },
    }),
  );
  return new URL(p.options.baseURL).hostname;
}

async function measure(kind: "node" | "rust", source: string) {
  const root = await mkdtemp(join(tmpdir(), `zcode-long-${kind}-`));
  await copyFile(source, join(root, "ts.sqlite"));
  const host = await providerFiles(root);
  const noProxy = `127.0.0.1,localhost,${host}`;
  const env = { NO_PROXY: noProxy, no_proxy: noProxy, ZCODE_NO_PROXY: noProxy };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          env,
        })
      : await fixture({
          root,
          registry: true,
          legacy: true,
          env,
          ...(rustBinary ? { binary: rustBinary } : {}),
        });
  const open = async () => {
    const t0 = performance.now();
    const h = f.start(workspace);
    await h.subscribe(`conversation/${session}`);
    await h.wait(
      (m) => m.method === "v4/conversation/frame" && m.params?.topic === `conversation/${session}`,
    );
    const openMs = performance.now() - t0;
    const t1 = performance.now();
    const page = await h.rows(session);
    const pageMs = performance.now() - t1;
    return { h, openMs, pageMs, pageRows: page.rows.length };
  };
  try {
    // 首次打开：Rust 含一次性 TS 导入；Node 直接读取。
    if (process.env.ZCODE_BENCH_DEBUG)
      console.log("STEP", kind, "open1", Math.round(performance.now()));
    const first = await open();
    if (process.env.ZCODE_BENCH_DEBUG)
      console.log("STEP", kind, "opened1", Math.round(performance.now()));
    await first.h.close();
    const second = await open();
    if (process.env.ZCODE_BENCH_DEBUG)
      console.log("STEP", kind, "opened2", Math.round(performance.now()));
    const h = second.h;
    const before = Math.max(
      0,
      ...((await h.rows(session)).rows as any[]).map((r) => Number(r.rowId) || 0),
    );
    const t0 = performance.now();
    const mark = h.messages.length;
    const ack = await h.command(
      // 与真实 App composer 一样随输入携带模型选择（Node 对导入的会话要求显式选择）。
      h.envelope("sendText", session, {
        text: "Reply with exactly one word: ok",
        mode: "yolo",
        modelSelection: {
          providerId: "personal:bench",
          modelId: benchModelId,
          options: { reasoningLevel: "high" },
        },
      }),
    );
    if (process.env.ZCODE_BENCH_DEBUG) {
      await new Promise((r) => setTimeout(r, 4000));
      const controls = h.messages
        .slice(mark)
        .flatMap((m) => m.params?.frame?.payload?.deltas ?? [])
        .map((d: any) => d.patch?.control)
        .filter(Boolean);
      console.log("ACK", kind, JSON.stringify(ack), JSON.stringify(controls.at(-1)));
    }
    await h.wait(
      (m) =>
        (m.params?.frame?.payload?.deltas ?? []).some(
          (d: any) =>
            (d.op === "row.delta" && Number(d.rowId) > before) ||
            ((d.row?.kind === "assistantText" || d.row?.kind === "reasoning") &&
              Number(d.row.rowId) > before &&
              String(d.row.text ?? "").length > 0),
        ),
      mark,
    );
    const firstOutputMs = performance.now() - t0;
    if (process.env.ZCODE_BENCH_DEBUG)
      console.log("STEP", kind, "firstOutput", Math.round(performance.now()));
    await h.completed(session, mark);
    const completeMs = performance.now() - t0;
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
    return {
      firstOpenMs: first.openMs,
      openMs: second.openMs,
      pageMs: second.pageMs,
      pageRows: second.pageRows,
      firstOutputMs,
      completeMs,
    };
  } finally {
    await f.close();
  }
}

test(
  "long history: Node vs Rust open, page and next-turn latency",
  { skip: !providerId && "set ZCODE_BENCH_PROVIDER_ID to run", timeout: 60 * 60_000 },
  async () => {
    const dir = await mkdtemp(join(tmpdir(), "zcode-long-seed-"));
    const source = join(dir, "ts.sqlite");
    const config = JSON.parse(
      await readFile(join(homedir(), ".zcode", "v2", "config.json"), "utf8"),
    );
    const provider = config.provider[providerId!];
    benchModelId = process.env.ZCODE_BENCH_MODEL ?? Object.keys(provider.models ?? {})[0]!;
    await seed(source, benchModelId);
    const samples: Record<"node" | "rust", Awaited<ReturnType<typeof measure>>[]> = {
      node: [],
      rust: [],
    };
    for (let i = 0; i < rounds; i++) {
      for (const kind of i % 2 === 0 ? (["node", "rust"] as const) : (["rust", "node"] as const)) {
        samples[kind].push(await measure(kind, source));
      }
    }
    const median = (v: number[]) =>
      Math.round([...v].sort((a, b) => a - b)[Math.floor(v.length / 2)]!);
    const max = (v: number[]) => Math.round(Math.max(...v));
    const summary = Object.fromEntries(
      (["node", "rust"] as const).map((kind) => {
        const s = samples[kind];
        const stat = (k: keyof (typeof s)[number]) => ({
          p50: median(s.map((x) => Number(x[k]))),
          max: max(s.map((x) => Number(x[k]))),
        });
        return [
          kind,
          {
            n: s.length,
            turns,
            pageRows: s[0]!.pageRows,
            firstOpenMs: stat("firstOpenMs"),
            openMs: stat("openMs"),
            pageMs: stat("pageMs"),
            firstOutputMs: stat("firstOutputMs"),
            completeMs: stat("completeMs"),
          },
        ];
      }),
    );
    console.log("LONG", JSON.stringify(summary));
  },
);
