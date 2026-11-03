import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const target = process.argv[2];
const readyMarkerNames = {
  main: ".main-build-ready",
  host: ".host-build-ready",
  preload: ".preload-build-ready",
};

if (!Object.hasOwn(readyMarkerNames, target)) {
  throw new Error(`unknown ready marker target: ${target ?? "<empty>"}`);
}

const readyMarkerPath = resolve(root, "out", readyMarkerNames[target]);

mkdirSync(dirname(readyMarkerPath), { recursive: true });

// tsup 的 CLI 级 onSuccess 会被每个子构建单独触发，不能代表 desktop 整体构建完成。
// 这里改成 main/host/preload 各自写独立 marker，dev.mjs 只有在三者都就绪后才会启动 Electron。
writeFileSync(readyMarkerPath, `${new Date().toISOString()}\n`, "utf8");
