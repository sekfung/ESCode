import { readdir, readFile } from "node:fs/promises";
import { relative, resolve } from "node:path";

import { countRustCodeLines } from "./rust-source-metrics.mjs";

// apps/escode-host 的边界与纪律检查（docs/specs/rust-u2-host-skeleton.md §3、§7）：
// 1) 有效代码行 ≤ 400（排除空行与整行注释；与 CLI 同规）；
// 2) 分层：host-core / host-proto 不得依赖 host-gateway / host-control；
//    任何 host-* 不得依赖 CLI 的 core/tools/model/state/wiki（Host 只经
//    core-api 公共 port、app-server 链路与 escode_cli_rust::embed 进入 runtime）；
// 3) 进程环境：非测试代码 0 处 std::env 读取——Host 的 env 由 Main 经控制消息传入
//    （RuntimeOptions），没有豁免文件。
const root = resolve(import.meta.dirname, "../apps/escode-host/crates");
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
    const codeLines = countRustCodeLines(source);
    if (codeLines > 400) failures.push(`${file}: ${codeLines} effective code lines exceeds 400`);
    if (file.includes("/tests/")) continue;
    const code = stripTestCode(source).replace(/\/\/[^\n]*/g, "");
    if (
      ["host-core", "host-proto"].includes(crate) &&
      /\bescode_host_(?:gateway|control)\s*::/.test(code)
    )
      failures.push(`${file}: base crate imports gateway or control`);
    if (/\bescode_cli_(?:core|tools|model|state|wiki)\s*::/.test(code))
      failures.push(`${file}: host imports CLI internals (use core-api/app-server/embed)`);
    if (
      /\bstd\s*::\s*env\s*::\s*(?:var|var_os|vars|vars_os|set_var|remove_var|current_dir|current_exe|args)\b|(?<![:\w])env\s*::\s*(?:var|var_os|vars|vars_os|set_var|remove_var|current_dir|current_exe|args)\s*\(/.test(
        code,
      )
    )
      failures.push(`${file}: reads process env (Host env arrives via control/RuntimeOptions)`);
  }
}
function stripTestCode(source) {
  const cfgTest = source.indexOf("#[cfg(test)]");
  return cfgTest === -1 ? source : source.slice(0, cfgTest);
}
await walk(root);
if (failures.length) {
  for (const failure of failures) console.error(failure);
  process.exitCode = 1;
} else console.log("Rust Host boundaries: OK");
