// Phase 3 golden freeze（CLI）：把 Node 侧观察冻结为仓内资产。
//
// 用法：
//   const node = await nodeOracle("case-name", () => observe("node"));
//   const rust = await observe("rust");
//
// 正常跑只执行 run()（Node）当 `ESCODE_RECORD_CLI_GOLDEN=1`；否则直接加载
// `packages/services/tests/fixtures/cli-golden/<suite>.<case>.json[.gz]`，不再启动 Node。
// 删除 Node/TS CLI 之前用一次 `ESCODE_RECORD_CLI_GOLDEN=1` 重录。
//
// `suite` 取当前测试文件名（去掉 `escode-cli-rust-` 前缀与 `.test.ts`），保证跨文件 key 不冲突。
import { deepEqual } from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { gzipSync, gunzipSync } from "node:zlib";

const goldenDir = resolve("packages/services/tests/fixtures/cli-golden");
const recording = process.env.ESCODE_RECORD_CLI_GOLDEN === "1";
const recordedThisRun = new Map<string, unknown>();
const suite = basename(process.env.ESCODE_GOLDEN_SUITE ?? process.argv[1] ?? "unknown", ".test.ts")
  .replace(/^escode-cli-rust-/, "")
  .replace(/\.test$/, "");

/** 去掉 undefined 键（JSON 冻结会丢它们；两侧同法归一后再比较）。 */
export function canonical<T>(value: T, parentKey?: string): T {
  if (typeof value === "string") {
    // 冻结观察里的环境事实：录制当天日期、内核版本、shell 错误前缀。跨机器重跑时同法归一。
    return (
      value
        .replace(/Today's date is \d{4}-\d{2}-\d{2}\./g, "Today's date is <DATE>.")
        .replace(/^(- Platform: ).*$/gm, "$1<platform>")
        .replace(/^(- OS Version: ).*$/gm, "$1<os>")
        // macOS 的临时目录是 /var/folders/.../T/；与 Linux /tmp/ 归一，避免同一条观察跨平台漂移。
        .replace(/\/var\/folders\/[^/\s]+\/[^/\s]+\/T\//g, "/tmp/")
        .replace(/Shell cwd was reset to .*/g, "Shell cwd was reset to <cwd>")
        // bash 的局部化错误文案（中文/英文）在 golden 与 CI 间不稳定；归一为同一占位。
        .replace(
          /(?:\/bin\/)?bash: (?:第 \d+ 行：|line \d+:)?\s*cd: missing-dir: .*/g,
          "<shell>: cd: missing-dir: <error>",
        )
        .replace(/(?:\/bin\/)?bash: line \d+:/g, "<shell>:") as T
    );
  }
  // JSON 把数组里的 undefined 写成 null；两侧同法归一（对象键仍按旧语义丢弃 undefined）。
  if (Array.isArray(value))
    return value.map((item) => (item === undefined ? null : canonical(item))) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([, item]) => item !== undefined)
        .map(([key, item]) => {
          // CPU 派生的并发上限随 runner 核数变化；Node/Rust 两侧同法归一为占位符。
          if (key === "concurrencyCeiling" || key === "ceiling") return [key, "<concurrency>"];
          if (parentKey === "concurrency" && key === "cap") return [key, "<concurrency>"];
          if (parentKey === "caps" && key === "maxConcurrency") return [key, "<concurrency>"];
          if (key === "x-client-language" || key === "x-client-timezone" || key === "x-os-version")
            return [key, "<env>"];
          return [key, canonical(item, key)];
        }),
    ) as T;
  }
  return value;
}

const platformKey = `${process.platform}-${process.arch}`;

function paths(name: string) {
  const stem = join(goldenDir, `${suite}.${name}`);
  return {
    // 平台专属观察优先：system prompt/路径/shell 错误等是环境事实，不能跨 OS 共用。
    platformPlain: `${stem}.${platformKey}.json`,
    platformGzip: `${stem}.${platformKey}.json.gz`,
    plain: `${stem}.json`,
    gzip: `${stem}.json.gz`,
  };
}

function loadExisting(file: ReturnType<typeof paths>): string | undefined {
  if (existsSync(file.platformGzip)) return file.platformGzip;
  if (existsSync(file.platformPlain)) return file.platformPlain;
  if (existsSync(file.gzip)) return file.gzip;
  if (existsSync(file.plain)) return file.plain;
  return undefined;
}

/** 该 key 是否已有冻结观察（录制模式恒为 true，便于首次录制）。 */
export function hasGolden(name: string): boolean {
  if (recording) return true;
  return loadExisting(paths(name)) !== undefined;
}

/** 超过 1 MiB 的观察（如内联附件正文）用 gzip 存储，避免仓库膨胀。 */
export async function nodeOracle<T>(name: string, run: () => Promise<T>): Promise<T> {
  const file = paths(name);
  if (recording) {
    const value = canonical(await run());
    const key = `${suite}.${name}`;
    if (recordedThisRun.has(key)) {
      deepEqual(
        recordedThisRun.get(key),
        value,
        `CLI golden key reused with different observation: ${key}`,
      );
    }
    recordedThisRun.set(key, value);
    const json = `${JSON.stringify(value, null, 2)}\n`;
    await mkdir(goldenDir, { recursive: true });
    if (json.length > 1024 * 1024) {
      await writeFile(file.gzip, gzipSync(Buffer.from(json)));
    } else {
      await writeFile(file.plain, json);
    }
    return value;
  }
  if (existsSync(file.gzip)) {
    return canonical(JSON.parse(gunzipSync(await readFile(file.gzip)).toString("utf8")) as T);
  }
  if (!existsSync(file.plain)) {
    throw new Error(
      `Missing CLI golden ${file.plain}; run once with ESCODE_RECORD_CLI_GOLDEN=1 to record.`,
    );
  }
  return canonical(JSON.parse(await readFile(file.plain, "utf8")) as T);
}
