// 把 src/server.mjs 产出到 dist/mcp/server.js。
//
// dist/ 与其它内置插件一样是构建产物（apps/escode-cli/.gitignore 忽略 dist/），宿主启动器约定
// 读 <pluginRoot>/dist/mcp/server.js，因此这一步必须发生在 seed / 打包之前 —— dev 链路见
// scripts/build-desktop-agent-cli.mjs 的 requiredDevPluginRuntimeBuilds，打包链路见
// packages/desktop/scripts/prepare-agent-node-bundle.mjs 的 requiresRuntime。
//
// 不需要 esbuild：入口没有构建期依赖，运行期才动态 import vendored REA。保持与上游同构的最小步骤。
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

const packageRoot = resolve(import.meta.dirname, "..");
const source = resolve(packageRoot, "src", "server.mjs");
const outfile = resolve(packageRoot, "dist", "mcp", "server.js");

await mkdir(dirname(outfile), { recursive: true });
await writeFile(outfile, await readFile(source));
console.log(`Built ${outfile}`);
