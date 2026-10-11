/** 目标平台（如 `win32-x64`）→ Rust target triple；`override` 非空时优先。 */
export function resolveRustTarget(platformKey: string, override?: string): string;
/** 目标操作系统对应的 Rust 二进制文件名（Windows 追加 `.exe`）。 */
export function rustBinaryFileName(os: string): string;
