import { dirname, join } from "node:path";

/** 默认的 Host 二进制路径：开发态取仓库的 target/debug，安装包内与主进程同级。
 *  `ESCODE_RUST_HOST_BINARY` 是 E2E/自定义安装布局的显式覆盖（不读其它业务 env）。 */
export function defaultRustHostBinary(
  appPath: string,
  packaged: boolean,
  env: NodeJS.ProcessEnv = process.env,
): string {
  const override = env.ESCODE_RUST_HOST_BINARY?.trim();
  if (override) return override;
  const name = process.platform === "win32" ? "escode-host.exe" : "escode-host";
  // 安装包：appPath 是 resources/app.asar；extraResources 落在 resources/escode-host/<binary>。
  if (packaged) return join(dirname(appPath), "escode-host", name);
  return join(appPath, "..", "..", "apps", "escode-host", "target", "debug", name);
}
