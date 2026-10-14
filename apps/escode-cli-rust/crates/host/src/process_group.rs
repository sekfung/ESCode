//! 工具子进程的会话与进程组隔离（docs/specs/rust-unified-host.md「实现质量对齐工作包」P0#3）。

/// 让子进程成为新会话与新进程组的首进程，与 Node `spawn({ detached: true })` 一致。
///
/// Unix：在 exec 前调用 `setsid()`；`setsid` 只在调用者已是进程组组长时返回 `EPERM`，此时退回
/// `setpgid(0, 0)`。不要与 `process_group(..)` 同时使用：先设进程组会让 `setsid` 失败。
/// 进程组 ID 等于子进程 pid，进程树回收（`crates/tools/src/process_tree.rs`）依赖这一点。
///
/// 修复：Bash / MCP / 自定义命令 / git 探测原先用 `process_group(0)`，子进程仍在原会话里、保留控制
/// 终端；runtime 在终端里运行时，读取 `/dev/tty` 的 ssh / sudo / git 凭据提示会一直等输入，直到
/// 超时。新会话没有控制终端，这类提示立即失败（与对标仓库 B7 修复同源）。
///
/// Windows 上没有对应操作（no-op）：进程树由 Job Object 负责（`crates/tools/src/win_job.rs`）。
pub fn isolate(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        // SAFETY: 闭包在 fork 之后、exec 之前运行于子进程中，只调用 async-signal-safe 的 setsid 与
        // setpgid 并读取 errno，不分配内存、不加锁，也不访问父进程的其他状态。
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() != -1 {
                    return Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EPERM) && libc::setpgid(0, 0) == 0 {
                    return Ok(());
                }
                Err(error)
            });
        }
    }
    #[cfg(not(unix))]
    let _ = command;
}
