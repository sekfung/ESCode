use crate::domain::prompt::GitSnapshot;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

// 只用于环境探测，输出与进程时间都有界；drop 也回收探测进程组，避免 Stop 留下 fsmonitor 等子进程。
#[cfg(unix)]
struct Group(u32);
#[cfg(unix)]
impl Drop for Group {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
async fn command(
    cwd: &Path,
    program: &str,
    args: &[&str],
    cancel: &CancellationToken,
) -> Option<String> {
    if cancel.is_cancelled() {
        return None;
    }
    let mut command = Command::new(program);
    crate::child_env::apply(&mut command, false);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().ok()?;
    #[cfg(unix)]
    let _group = Group(child.id()?);
    #[cfg(windows)]
    let pid = child.id()?;
    let mut stdout = child.stdout.take()?.take(1024 * 1024 + 1);
    let read = async {
        let mut bytes = vec![];
        stdout.read_to_end(&mut bytes).await.ok()?;
        if bytes.len() > 1024 * 1024 || !child.wait().await.ok()?.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    };
    let result = tokio::select! { biased;
        _=cancel.cancelled()=>None,
        _=tokio::time::sleep(Duration::from_secs(3))=>None,
        result=read=>result,
    };
    #[cfg(windows)]
    if child.try_wait().ok().flatten().is_none() {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await;
        let _ = child.kill().await;
    }
    result
}
async fn git(cwd: &Path, args: &[&str], cancel: &CancellationToken) -> String {
    command(cwd, "git", args, cancel)
        .await
        .unwrap_or_default()
        .trim()
        .into()
}
async fn may_have_git(cwd: &Path, cancel: &CancellationToken) -> bool {
    if std::env::var_os("GIT_DIR").is_some() || std::env::var_os("GIT_WORK_TREE").is_some() {
        return true;
    }
    let cwd = match crate::realpath(cwd).await {
        Ok(path) => path,
        Err(_) => return true,
    };
    for dir in cwd.ancestors() {
        if cancel.is_cancelled() {
            return false;
        }
        match tokio::fs::symlink_metadata(dir.join(".git")).await {
            Ok(_) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return true,
        }
    }
    false
}
pub(super) async fn snapshot(cwd: &Path, cancel: &CancellationToken) -> Option<GitSnapshot> {
    // macOS 的系统 git 启动成本显著；不存在仓库标记时无需启动探测进程。显式 Git 环境仍交给 Git。
    if !may_have_git(cwd, cancel).await {
        return None;
    }
    if git(cwd, &["rev-parse", "--is-inside-work-tree"], cancel).await != "true" {
        return None;
    }
    let (branch, main_branch, user, status, recent) = tokio::join!(
        git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"], cancel),
        main_branch(cwd, cancel),
        git(cwd, &["config", "user.name"], cancel),
        git(cwd, &["--no-optional-locks", "status", "--short"], cancel),
        git(
            cwd,
            &["--no-optional-locks", "log", "--oneline", "-n", "5"],
            cancel
        ),
    );
    let status = if status.encode_utf16().count() > 2000 {
        let units: Vec<_> = status.encode_utf16().take(2000).collect();
        format!(
            "{}\n... (truncated because it exceeds 2k characters. If you need more information, run \"git status\" using {})",
            String::from_utf16_lossy(&units),
            if cfg!(windows) { "PowerShell" } else { "Bash" }
        )
    } else {
        status
    };
    Some(GitSnapshot {
        branch: if branch.is_empty() {
            "HEAD".into()
        } else {
            branch
        },
        main_branch,
        user,
        status: status.replace("\r\n", "\n"),
        recent_commits: recent
            .lines()
            .filter(|line| !line.is_empty())
            .take(5)
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n"),
    })
}
async fn main_branch(cwd: &Path, cancel: &CancellationToken) -> String {
    let remote = git(
        cwd,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        cancel,
    )
    .await;
    for name in [
        remote.strip_prefix("origin/").unwrap_or(&remote),
        "main",
        "master",
    ] {
        if !name.is_empty()
            && command(
                cwd,
                "git",
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/origin/{name}"),
                ],
                cancel,
            )
            .await
            .is_some()
        {
            return name.into();
        }
    }
    "main".into()
}
pub(super) async fn os_release(cwd: &Path, cancel: &CancellationToken) -> String {
    #[cfg(windows)]
    {
        // Windows 不 spawn 子进程，参数只为与 POSIX 分支保持同一签名。
        let _ = (cwd, cancel);
        windows_os_release().unwrap_or_else(|| "unknown".into())
    }
    #[cfg(not(windows))]
    {
        command(cwd, "uname", &["-r"], cancel)
            .await
            .unwrap_or_else(|| "unknown".into())
    }
}

/// 与 Node `os.release()` 一致：major.minor.build（如 10.0.26100）。
/// 读注册表而不是启动 PowerShell：本机一次 PowerShell 启动约 1.5s，而这段在 prompt 快照的关键路径上，
/// 会让首个模型请求延迟到秒级（也让依赖「1s 内发出请求」的用例必然超时）。
#[cfg(windows)]
fn windows_os_release() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    const SUBKEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let read_dword = |name: &str| -> Option<u32> {
        let (subkey, value) = (wide(SUBKEY), wide(name));
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut data).cast(),
                &mut size,
            )
        };
        (status == 0).then_some(data)
    };
    let read_string = |name: &str| -> Option<String> {
        let (subkey, value) = (wide(SUBKEY), wide(name));
        let mut size = 0u32;
        let probe = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if probe != 0 || size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; size.div_ceil(2) as usize];
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        String::from_utf16(&buffer[..end]).ok()
    };
    let major = read_dword("CurrentMajorVersionNumber")?;
    let minor = read_dword("CurrentMinorVersionNumber")?;
    let build = read_string("CurrentBuildNumber")?;
    (!build.is_empty()).then(|| format!("{major}.{minor}.{build}"))
}
