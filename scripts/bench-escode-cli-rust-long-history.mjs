import { spawn } from "node:child_process";
import { realpathSync } from "node:fs";
import { appendFile, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

// 与 test-escode-cli-rust.mjs 同一处理：Windows runner 的 TEMP 可能是 8.3 短路径，
// Node 与 Rust 子进程混用短名/长名会让路径归一化和 fs.watch 出现假差异。
if (process.platform === "win32") {
  const longTemp = realpathSync.native(tmpdir());
  process.env.TEMP = longTemp;
  process.env.TMP = longTemp;
}

const root = resolve(import.meta.dirname, "..");
const outputDir = resolve(root, process.argv[2] ?? "bench-results/long-history");
const turns = process.env.ESCODE_BENCH_HISTORY_TURNS ?? "20000";
const rustBinary =
  process.env.ESCODE_BENCH_RUST_BINARY ??
  resolve(
    `apps/escode-cli-rust/target/release/escode-cli-rust${process.platform === "win32" ? ".exe" : ""}`,
  );
const env = {
  ...process.env,
  // 大历史只在显式 benchmark 中运行；fixture 让同一 workload 不依赖真实供应商。
  ESCODE_BENCH_FIXTURE: "1",
  ESCODE_BENCH_HISTORY_TURNS: turns,
  ESCODE_BENCH_ROUNDS: "1",
  ESCODE_BENCH_SKIP_TURN: "1",
  ESCODE_BENCH_RUST_BINARY: rustBinary,
  TSX_TSCONFIG_PATH: resolve(root, "packages/services/tests/tsconfig.escode-cli-rust.json"),
  // 20k 轮的首次导入与进程关闭在慢 runner 上会超过普通用例预算。
  ESCODE_TEST_REQUEST_MS: process.env.ESCODE_TEST_REQUEST_MS ?? "120000",
  ESCODE_TEST_EXIT_MS: process.env.ESCODE_TEST_EXIT_MS ?? "60000",
};

await mkdir(outputDir, { recursive: true });
const logPath = join(outputDir, "output.log");
const summaryPath = join(outputDir, "summary.json");
const markdownPath = join(outputDir, "summary.md");

const child = spawn(
  process.execPath,
  [
    "--import",
    "tsx",
    "--test",
    "--test-timeout=1800000",
    "--test-force-exit",
    "packages/services/tests/escode-cli-rust-long-history-bench.test.ts",
  ],
  { cwd: root, env, stdio: ["ignore", "pipe", "pipe"] },
);

const chunks = [];
child.stdout.on("data", (chunk) => {
  process.stdout.write(chunk);
  chunks.push(chunk);
});
child.stderr.on("data", (chunk) => {
  process.stderr.write(chunk);
  chunks.push(chunk);
});
const code = await new Promise((done, fail) => {
  child.once("error", fail);
  child.once("close", done);
});
const output = Buffer.concat(chunks).toString("utf8");
await writeFile(logPath, output);
if (code !== 0) process.exit(code ?? 1);

const line = output
  .split(/\r?\n/)
  .reverse()
  .find((entry) => entry.startsWith("LONG "));
if (!line) throw new Error("long-history benchmark did not emit a LONG summary");
const summary = JSON.parse(line.slice("LONG ".length));
await writeFile(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);
const markdown = renderSummary(summary);
await writeFile(markdownPath, markdown);
if (process.env.GITHUB_STEP_SUMMARY) await appendFile(process.env.GITHUB_STEP_SUMMARY, markdown);

function metric(sample, key) {
  const value = sample?.[key]?.p50;
  return typeof value === "number" ? String(Math.round(value)) : "-";
}

function renderSummary(value) {
  const node = value.samples?.node;
  const rust = value.samples?.rust;
  return [
    `### Rust long history (${value.platform}, ${value.turns} turns)`,
    "",
    "| metric | Node | Rust release |",
    "| --- | ---: | ---: |",
    `| first open (Rust includes import) | ${metric(node, "firstOpenMs")} ms | ${metric(rust, "firstOpenMs")} ms |`,
    `| second open | ${metric(node, "openMs")} ms | ${metric(rust, "openMs")} ms |`,
    `| rows page | ${metric(node, "pageMs")} ms | ${metric(rust, "pageMs")} ms |`,
    `| page rows | ${node?.pageRows ?? "-"} | ${rust?.pageRows ?? "-"} |`,
    "",
    `schema errors: asserted 0 on both runtimes; Node ${value.node}; next turn skipped.`,
    "",
  ].join("\n");
}
