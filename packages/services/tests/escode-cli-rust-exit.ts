/**
 * Node 24 在 Windows 上退出时偶发 libuv 断言 `!(handle->flags & UV_HANDLE_CLOSING)`（src/win/async.c，
 * 退出码 0xC0000409），差分用例的 Node 一侧因此失败（CI 36213030115、36216871411）。该断言文本只可能来自
 * Node/libuv，Rust 子进程不会输出；只识别这一种退出，其余非零退出照常失败。
 */
export function isKnownNodeExitCrash(status: unknown, stderr: string): boolean {
  return (
    process.platform === "win32" &&
    Array.isArray(status) &&
    status[0] === 0xc0000409 &&
    stderr.includes("UV_HANDLE_CLOSING")
  );
}
