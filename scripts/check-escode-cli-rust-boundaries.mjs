import { readdir, readFile } from "node:fs/promises";
import { resolve, relative } from "node:path";

const root = resolve(import.meta.dirname, "../apps/escode-cli-rust/crates");
const failures = [];
async function walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) {
      await walk(path);
      continue;
    }
    if (!path.endsWith(".rs")) continue;
    const source = await readFile(path, "utf8");
    const file = relative(root, path).replaceAll("\\", "/");
    const crate = file.split("/", 1)[0];
    const code = source.replace(/\/\/[^\n]*/g, "");
    if (source.split("\n").length > 400) failures.push(`${file}: exceeds 400 lines`);
    if (
      crate === "domain" &&
      /\b(?:tokio|reqwest|rusqlite|escode_cli_(?:state|model|tools|host|wiki))\s*::|\bstd\s*::\s*(?:fs|process|net)\b/.test(
        code,
      )
    )
      failures.push(`${file}: domain imports runtime or IO`);
    if (
      crate === "protocol" &&
      /\bescode_cli_(?:core|core_api|state|model|tools|host)\s*::/.test(code)
    )
      failures.push(`${file}: protocol imports core or adapters`);
    if (
      crate === "core" &&
      /\bescode_cli_(?:state|model|tools|host|wiki|app_server|tui)\s*::/.test(code)
    )
      failures.push(`${file}: core imports an adapter or frontend`);
    if (crate === "tui" && /\bescode_cli_(?:state|model|tools|host|wiki)\s*::/.test(code))
      failures.push(`${file}: tui imports an adapter`);
    if (
      // wiki：LLM Wiki 适配器（apps/escode-cli-rust/docs/llm-wiki）；net：网络出口适配器（P1 #6）。
      ["state", "model", "tools", "host", "wiki", "net"].includes(crate) &&
      /\bescode_cli_(?:core|app_server|tui)\s*::/.test(code)
    )
      failures.push(`${file}: adapter imports core or frontend`);
    // 显式环境（docs/specs/rust-u1-runtime-lib.md §3.5）：非测试代码 0 处 std::env 进程事实读取。
    // 唯一豁免 core-api/src/env.rs（被批准的进程事实采集 shim）；temp_dir 是进程级语义，允许。
    if (file.includes("/tests/")) continue;
    const runtimeCode = stripTestCode(source).replace(/\/\/[^\n]*/g, "");
    if (
      (crate !== "core-api" || file !== "core-api/src/env.rs") &&
      /\bstd\s*::\s*env\s*::\s*(?:var|var_os|vars|vars_os|set_var|remove_var|current_dir|current_exe|args)\b|(?<![:\w])env\s*::\s*(?:var|var_os|vars|vars_os|set_var|remove_var|current_dir|current_exe|args)\s*\(/.test(
        runtimeCode,
      )
    )
      failures.push(`${file}: reads process env outside core-api/src/env.rs`);
  }
}
/// 剔除 tests/ 目录文件与 `#[cfg(test)]` 段之后的内容（测试代码允许 std::env）。
function stripTestCode(source) {
  const cfgTest = source.indexOf("#[cfg(test)]");
  return cfgTest === -1 ? source : source.slice(0, cfgTest);
}
await walk(root);
if (failures.length) {
  for (const failure of failures) console.error(failure);
  process.exitCode = 1;
} else console.log("Rust workspace boundaries: OK");
