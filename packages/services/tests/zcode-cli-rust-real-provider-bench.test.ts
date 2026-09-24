import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture } from "./zcode-cli-rust-fixture.js";

// 性能验收（docs/reports/rust-perf-*）：真实供应商下 Node 与 Rust 的首段延迟与完成延迟分布。
// 默认跳过；ZCODE_BENCH_PROVIDER_ID=<~/.zcode/v2/config.json 中的 provider id> 时运行。
// 两侧使用同一份 personal provider 配置（密钥运行时读取，不打印），交替执行以抵消供应商侧漂移。
const providerId = process.env.ZCODE_BENCH_PROVIDER_ID;
const rounds = Number(process.env.ZCODE_BENCH_ROUNDS ?? 20);
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
// 场景：short 一词回复；long 约 300 词的流式长回复；tool 先调用 Read 再回答（统计工具是否成功）。
const scenario = (process.env.ZCODE_BENCH_SCENARIO ?? "short") as "short" | "long" | "tool";
const prompts = {
  short: ["Reply with exactly one word: ok", "Reply with exactly one word: yes"],
  long: [
    "Write about 300 words describing a lighthouse at night. Plain prose, no lists.",
    "Write about 300 words describing a harbor at dawn. Plain prose, no lists.",
  ],
  tool: [
    "Use the Read tool to read sample.txt, then reply with only its first word.",
    "Use the Read tool to read sample.txt again, then reply with only its last word.",
  ],
}[scenario];
// release 构建：ZCODE_BENCH_RUST_BINARY 指向 cargo --release 产物；缺省用测试夹具的 debug 构建。
const rustBinary = process.env.ZCODE_BENCH_RUST_BINARY;

function percentile(values: number[], p: number) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)]!;
}

async function providerConfig(root: string) {
  const config = JSON.parse(await readFile(join(homedir(), ".zcode", "v2", "config.json"), "utf8"));
  const p = config.provider?.[providerId!];
  assert(p?.options?.baseURL, "provider needs baseURL");
  const modelId = process.env.ZCODE_BENCH_MODEL ?? Object.keys(p.models ?? {})[0];
  assert(modelId, "provider has no model");
  const api =
    p.kind === "anthropic"
      ? { type: "anthropic-messages", baseUrl: p.options.baseURL }
      : { type: "openai-chat-completions", baseUrl: p.options.baseURL };
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
                // 自建/局域网服务可以不设密钥；两侧同样只发送占位值。
                access: { type: "api-key", apiKey: p.options.apiKey || "none" },
                api,
                personalModelIds: [modelId],
              },
            },
          ],
        },
        modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
        defaultModelSelection: { providerId: "personal:bench", modelId },
      },
    }),
  );
  return { modelId, host: new URL(p.options.baseURL).hostname };
}

/** 供应商主机加入两侧的 NO_PROXY：开发机代理会把局域网请求转走（实测返回 503）。 */
async function providerHost() {
  const config = JSON.parse(await readFile(join(homedir(), ".zcode", "v2", "config.json"), "utf8"));
  return new URL(config.provider[providerId!].options.baseURL).hostname;
}

async function measure(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-bench-${kind}-`));
  const noProxy = `127.0.0.1,localhost,${await providerHost()}`;
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
      : await fixture({ root, registry: true, env, ...(rustBinary ? { binary: rustBinary } : {}) });
  await providerConfig(root);
  await writeFile(join(f.cwd, "sample.txt"), "alpha beta gamma delta\n");
  const started = performance.now();
  const h = f.start();
  try {
    const id = await h.create();
    const ready = performance.now() - started;
    await h.subscribe(`conversation/${id}`);
    // 同一会话两轮：首轮含进程内首次请求的固定开销（冷），第二轮代表日常使用（热）。
    const turn = async (text: string) => {
      // 只认本轮新建的行：两侧都会在后续增量里重发上一轮的行（upsert），不能算作首段文本。
      const before = Math.max(
        0,
        ...((await h.rows(id)).rows as Record<string, any>[]).map((r) => Number(r.rowId) || 0),
      );
      const t0 = performance.now();
      const mark = h.messages.length;
      await h.command(h.envelope("sendText", id, { text, mode: "yolo" }));
      await h.wait(
        (m) =>
          (m.params?.frame?.payload?.deltas ?? []).some(
            (d: any) =>
              (d.op === "row.delta" && Number(d.rowId) > before) ||
              (d.row?.kind === "assistantText" &&
                Number(d.row.rowId) > before &&
                String(d.row.text ?? "").length > 0),
          ),
        mark,
      );
      const firstMs = performance.now() - t0;
      if (process.env.ZCODE_BENCH_TRACE) {
        // 诊断：本轮每种行首次出现的时刻（按帧到达顺序），用于解释首段文本延迟的差异。
        await h.completed(id, mark);
        const seen: Record<string, number> = {};
        for (const m of h.messages.slice(mark)) {
          for (const d of m.params?.frame?.payload?.deltas ?? []) {
            const rowId = Number(d.row?.rowId ?? d.rowId);
            if (!(rowId > before)) continue;
            const kind = d.row?.kind ?? `delta:${rowId}`;
            const at = Math.round((m.__receivedAt ?? 0) - t0);
            if (seen[kind] === undefined) seen[kind] = at;
          }
        }
        console.log("TRACE", kind, JSON.stringify(seen));
      }
      await h.completed(id, mark);
      return { firstMs, totalMs: performance.now() - t0 };
    };
    const cold = await turn(prompts[0]!);
    const warm = await turn(prompts[1]!);
    const rows = (await h.rows(id)).rows as Record<string, any>[];
    const answered =
      rows.filter((r) => r.kind === "assistantText" && String(r.text ?? "").length > 0).length >= 2;
    const tools = rows.filter((r) => r.kind === "toolCall");
    const toolsOk = tools.length > 0 && tools.every((r) => r.status === "success");
    const answerChars = rows
      .filter((r) => r.kind === "assistantText")
      .reduce((n, r) => n + String(r.text ?? "").length, 0);
    assert.deepEqual(h.schemaErrors, []);
    return {
      ready,
      firstMs: cold.firstMs,
      totalMs: cold.totalMs,
      warmFirstMs: warm.firstMs,
      warmTotalMs: warm.totalMs,
      answered,
      toolsOk,
      answerChars,
    };
  } finally {
    await h.close().catch(() => undefined);
    await f.close();
  }
}

test(
  "real provider latency: Node vs Rust (first visible text and completion)",
  {
    skip: !providerId && "set ZCODE_BENCH_PROVIDER_ID to call a real provider",
    timeout: 30 * 60_000,
  },
  async () => {
    const samples = {
      node: [] as Awaited<ReturnType<typeof measure>>[],
      rust: [] as Awaited<ReturnType<typeof measure>>[],
    };
    for (let i = 0; i < rounds; i++) {
      // 交替先后顺序，抵消供应商侧随时间的漂移。
      for (const kind of i % 2 === 0 ? (["node", "rust"] as const) : (["rust", "node"] as const)) {
        samples[kind].push(await measure(kind));
      }
    }
    const summary = Object.fromEntries(
      (["node", "rust"] as const).map((kind) => {
        const s = samples[kind];
        const pick = (key: "ready" | "firstMs" | "totalMs" | "warmFirstMs" | "warmTotalMs") =>
          s.map((x) => x[key]);
        const stats = (values: number[]) => ({
          p50: Math.round(percentile(values, 50)),
          p95: Math.round(percentile(values, 95)),
          p99: Math.round(percentile(values, 99)),
        });
        return [
          kind,
          {
            n: s.length,
            scenario,
            toolsOk: s.filter((x) => x.toolsOk).length,
            answerChars: Math.round(s.reduce((n, x) => n + x.answerChars, 0) / s.length),
            answered: s.filter((x) => x.answered).length,
            readyMs: stats(pick("ready")),
            firstTextMs: stats(pick("firstMs")),
            completeMs: stats(pick("totalMs")),
            warmFirstTextMs: stats(pick("warmFirstMs")),
            warmCompleteMs: stats(pick("warmTotalMs")),
          },
        ];
      }),
    );
    console.log("BENCH", JSON.stringify(summary));
    // 长回复下模型本身偶发只推理不作答或自发调用工具；计数写入报告，低于 90% 才视为失败。
    assert(summary.node!.answered >= rounds * 0.9, "node answered too few turns");
    assert(summary.rust!.answered >= rounds * 0.9, "rust answered too few turns");
  },
);
