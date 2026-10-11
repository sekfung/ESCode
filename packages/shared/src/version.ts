// 由各 bundler 通过 define 注入，避免运行时 JSON import 的跨 bundler 兼容问题。
// 非构建环境（如 e2e 测试的 mocha）下 define 不存在，
// 用 typeof 检查 + fallback 避免 ReferenceError。
declare const __ESCODE_VERSION__: string;
declare const __ESCODE_COMMIT__: string;
declare const __ESCODE_BUILD_TIME__: string;

export const ESCODE_VERSION: string =
  typeof __ESCODE_VERSION__ !== "undefined" ? __ESCODE_VERSION__ : "0.0.0-dev";
export const ESCODE_COMMIT: string =
  typeof __ESCODE_COMMIT__ !== "undefined" ? __ESCODE_COMMIT__ : "unknown";
export const ESCODE_BUILD_TIME: string =
  typeof __ESCODE_BUILD_TIME__ !== "undefined" ? __ESCODE_BUILD_TIME__ : "unknown";
