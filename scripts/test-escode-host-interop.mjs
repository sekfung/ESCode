import { spawn } from "node:child_process";
import { resolve } from "node:path";

// 双 Host settings 交替写读演练（docs/specs/rust-u2-host-skeleton.md §5 优先序 3 验收）：
// 先构建 escode-host，再用 Node 内置 test runner 跑 packages/services 的互操作用例。
// 与差分套件同口径：TS 源码经 tsx 直接导入，需要 TSX_TSCONFIG_PATH 指向 services 的测试 tsconfig。
const root = resolve(import.meta.dirname, "..");
const tests = ["packages/services/tests/escode-settings-host-interop.test.ts"];

function run(command, args, env = {}) {
  return new Promise((done, fail) => {
    const child = spawn(command, args, {
      cwd: root,
      stdio: "inherit",
      env: { ...process.env, ...env },
    });
    child.once("error", fail);
    child.once("close", (code, signal) =>
      code === 0 ? done() : fail(new Error(`${command} failed: ${signal ?? code}`)),
    );
  });
}

await run("cargo", ["build", "--manifest-path", "apps/escode-host/Cargo.toml", "--bin", "escode-host"]);
await run(
  process.execPath,
  ["--import", "tsx", "--test", "--test-timeout=300000", "--test-force-exit", ...tests],
  {
    TSX_TSCONFIG_PATH: resolve(root, "packages/services/tests/tsconfig.escode-cli-rust.json"),
  },
);
