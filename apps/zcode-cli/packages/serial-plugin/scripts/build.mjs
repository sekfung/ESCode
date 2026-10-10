import { mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { build } from "esbuild";

const packageRoot = resolve(import.meta.dirname, "..");
const outfile = resolve(packageRoot, "dist", "mcp", "server.js");

// 与 node-repl-host 相同：esm 产物里的 __require shim 在 ESM 作用域没有 require，
// 依赖链里的 CJS 包会在模块求值阶段抛错，插件宿主 import 失败、注册 0 个工具。注入真实 createRequire。
const nodeRequireBanner = `import { createRequire as __zcodeCreateRequire } from "node:module";
const require = __zcodeCreateRequire(import.meta.url);`;

await mkdir(dirname(outfile), { recursive: true });
await build({
  banner: { js: nodeRequireBanner },
  bundle: true,
  entryPoints: [resolve(packageRoot, "src", "server.ts")],
  format: "esm",
  legalComments: "none",
  outfile,
  platform: "node",
  target: "node24",
});
console.log(`Built ${outfile}`);
