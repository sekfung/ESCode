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
    if (crate === "core" && /\bescode_cli_(?:state|model|tools|host|wiki|app_server|tui)\s*::/.test(code))
      failures.push(`${file}: core imports an adapter or frontend`);
    if (crate === "tui" && /\bescode_cli_(?:state|model|tools|host|wiki)\s*::/.test(code))
      failures.push(`${file}: tui imports an adapter`);
    if (
      // wiki：LLM Wiki 适配器（apps/escode-cli-rust/docs/llm-wiki），与其他 adapter 同一约束。
      ["state", "model", "tools", "host", "wiki"].includes(crate) &&
      /\bescode_cli_(?:core|app_server|tui)\s*::/.test(code)
    )
      failures.push(`${file}: adapter imports core or frontend`);
  }
}
await walk(root);
if (failures.length) {
  for (const failure of failures) console.error(failure);
  process.exitCode = 1;
} else console.log("Rust workspace boundaries: OK");
