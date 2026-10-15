//! `terminal` 域（docs/specs/rust-u2-terminal.md；TS 真相源 `packages/services/src/terminal/`）。
//!
//! Host 内唯一的 [`TerminalService`] 持有全部 pty：终端属于 Host 而不属于连接。事件语义照搬 TS `Emitter`
//! （只投递订阅之后的数据、dispose 不再投递 exit），背压比 TS 更严格（见 spec §2）。

mod jsonc;
mod launch;
mod profile;
mod pump;
mod stream;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};

use launch::Env;
use pump::{Listeners, Pump};
use stream::Utf8Decoder;

/// `create` 的入参（TS `{cols, rows, cwd?}`）。
pub struct CreateParams {
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<String>,
}

struct Terminal {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    listeners: Arc<Listeners>,
    cancel: watch::Sender<bool>,
}

/// Host 内唯一的 terminal 服务。
#[derive(Default)]
pub struct TerminalService {
    next_id: AtomicU64,
    terminals: Arc<Mutex<HashMap<String, Arc<Terminal>>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn not_found(id: &str) -> anyhow::Error {
    anyhow::anyhow!("Terminal not found: {id}")
}

impl TerminalService {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, id: &str) -> anyhow::Result<Arc<Terminal>> {
        lock(&self.terminals)
            .get(id)
            .cloned()
            .ok_or_else(|| not_found(id))
    }

    /// TS `create`。`env` 是 Host 进程 env 快照，`settings` 是 settings 域的当前值（读失败时调用方传空对象）。
    pub async fn create(
        &self,
        params: CreateParams,
        env: &Env,
        settings: &Value,
    ) -> anyhow::Result<Value> {
        // TS 先领号再解析：启动失败也会消耗一个 id。
        let id = self.next_id.fetch_add(1, Ordering::SeqCst).to_string();
        let windows = cfg!(windows);
        let shell = launch::terminal_shell(env, windows).await?;
        let cwd = launch::terminal_cwd(env, params.cwd.as_deref()).await?;
        let terminal_env = launch::terminal_env(env, cfg!(target_os = "macos"));
        let font = profile::resolve_font_profile(
            settings.get("terminalFontFamily").and_then(Value::as_str),
            settings.get("terminalInheritSystemProfile") != Some(&Value::Bool(false)),
            env,
            profile::platform(),
        )
        .await;

        let spawned = {
            let (shell, cwd) = (shell.clone(), cwd.clone());
            tokio::task::spawn_blocking(move || {
                spawn(&shell, &cwd, terminal_env, params.cols, params.rows)
            })
            .await?
        }
        .map_err(|error| {
            anyhow::anyhow!("Failed to start terminal with shell '{shell}' in '{cwd}': {error:#}")
        })?;

        let terminal = Arc::new(spawned.terminal);
        lock(&self.terminals).insert(id.clone(), Arc::clone(&terminal));
        Pump {
            id: id.clone(),
            terminal_listeners: Arc::clone(&terminal.listeners),
            cancel: terminal.cancel.subscribe(),
            chunks: spawned.chunks,
            exit: spawned.exit,
            on_exit: {
                let terminals = Arc::clone(&self.terminals);
                let terminal = Arc::clone(&terminal);
                Box::new(move |id: &str| {
                    // 自然退出：只摘除自己（同 id 不会被复用，但保持幂等）。
                    let mut table = lock(&terminals);
                    if table
                        .get(id)
                        .is_some_and(|current| Arc::ptr_eq(current, &terminal))
                    {
                        table.remove(id);
                    }
                })
            },
        }
        .spawn();

        let mut result = json!({
            "id": id,
            "shell": shell,
            "fontFamily": font.font_family,
            "fontFamilySource": font.source,
        });
        if let Some(size) = font.font_size {
            result["fontSize"] = json!(size);
        }
        if let Some(theme) = font.theme {
            result["theme"] = theme;
        }
        if let Some(info) = windows_pty_info(windows).await {
            result["windowsPty"] = info;
        }
        Ok(result)
    }

    /// TS `write`：写入可能因 pty 缓冲满而阻塞，放到阻塞线程池。
    pub async fn write(&self, id: &str, data: String) -> anyhow::Result<()> {
        let writer = Arc::clone(&self.get(id)?.writer);
        tokio::task::spawn_blocking(move || {
            let mut writer = lock(&writer);
            writer.write_all(data.as_bytes())?;
            writer.flush()
        })
        .await??;
        Ok(())
    }

    /// TS `resize`。
    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        let terminal = self.get(id)?;
        lock(&terminal.master).resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    }

    /// TS `dispose`（`cleanupTerminal`）：kill、摘除监听者、删表；不再投递 exit；未知 id 静默成功。
    pub async fn dispose(&self, id: &str) {
        let Some(terminal) = lock(&self.terminals).remove(id) else {
            return;
        };
        let _ = lock(&terminal.killer).kill();
        let _ = terminal.cancel.send(true);
        terminal.listeners.clear();
    }

    /// TS `onDynamicData(id)`：未知 id 抛错（调用方只记日志）。
    pub fn listen_data(&self, id: &str) -> anyhow::Result<mpsc::Receiver<String>> {
        Ok(self.get(id)?.listeners.add_data())
    }

    /// TS `onDynamicExit(id)`。
    pub fn listen_exit(&self, id: &str) -> anyhow::Result<mpsc::Receiver<i64>> {
        Ok(self.get(id)?.listeners.add_exit())
    }
}

struct Spawned {
    terminal: Terminal,
    chunks: mpsc::Receiver<String>,
    exit: oneshot::Receiver<i64>,
}

/// 开 pty、启动 shell，并起读线程与等待线程（阻塞 IO 都在专用线程里，不占 tokio worker）。
fn spawn(
    shell: &str,
    cwd: &str,
    env: std::collections::BTreeMap<String, String>,
    cols: u16,
    rows: u16,
) -> anyhow::Result<Spawned> {
    let pair = native_pty_system().openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(shell);
    command.env_clear();
    for (key, value) in env {
        command.env(key, value);
    }
    command.cwd(cwd);
    let mut child = pair.slave.spawn_command(command)?;
    // 必须释放 slave：否则子进程退出后 master 读端永远不会 EOF。
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let killer = child.clone_killer();

    let (chunk_tx, chunks) = mpsc::channel::<String>(64);
    std::thread::Builder::new()
        .name("escode-terminal-read".to_owned())
        .spawn(move || {
            let mut decoder = Utf8Decoder::default();
            let mut buffer = vec![0_u8; 16 * 1024];
            // 读端出错（Linux 上 slave 全部关闭时为 EIO）即结束，等同 EOF。
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let text = decoder.push(&buffer[..count]);
                // 有界通道：消费者慢时在这里阻塞，pty 缓冲满后子进程写阻塞；接收端被丢弃（dispose）即退出。
                if !text.is_empty() && chunk_tx.blocking_send(text).is_err() {
                    break;
                }
            }
        })?;

    let (exit_tx, exit) = oneshot::channel::<i64>();
    std::thread::Builder::new()
        .name("escode-terminal-wait".to_owned())
        .spawn(move || {
            // node-pty 对被信号终止的进程报 exitCode=0（信号另给）；portable-pty 此时给 1。
            let code = child.wait().map_or(0, |status| {
                if status.signal().is_some() {
                    0
                } else {
                    i64::from(status.exit_code())
                }
            });
            let _ = exit_tx.send(code);
        })?;

    let (cancel, _) = watch::channel(false);
    Ok(Spawned {
        terminal: Terminal {
            master: Mutex::new(pair.master),
            writer: Arc::new(Mutex::new(writer)),
            killer: Mutex::new(killer),
            listeners: Arc::new(Listeners::default()),
            cancel,
        },
        chunks,
        exit,
    })
}

/// TS `resolveTerminalWindowsPtyInfo`：仅 Windows；buildNumber 为 OS release 第三段（经 `ver` 取得）。
async fn windows_pty_info(windows: bool) -> Option<Value> {
    if !windows {
        return None;
    }
    let output = tokio::process::Command::new("cmd")
        .args(["/C", "ver"])
        .output()
        .await
        .ok();
    let build =
        output.and_then(|output| build_number_from_ver(&String::from_utf8_lossy(&output.stdout)));
    Some(match build {
        Some(build) => json!({"backend": "conpty", "buildNumber": build}),
        None => json!({"backend": "conpty"}),
    })
}

/// `Microsoft Windows [Version 10.0.22631.4602]` → `22631`（TS `parseWindowsBuildNumber(os.release())`）。
fn build_number_from_ver(text: &str) -> Option<i64> {
    let version = text.split("Version").nth(1)?;
    let version = version
        .trim_start()
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()?;
    version.split('.').nth(2)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::build_number_from_ver;

    #[test]
    fn windows_build_number_comes_from_third_version_segment() {
        assert_eq!(
            build_number_from_ver("\r\nMicrosoft Windows [Version 10.0.22631.4602]\r\n"),
            Some(22631)
        );
        assert_eq!(build_number_from_ver("no version here"), None);
        assert_eq!(build_number_from_ver("[Version 10.0]"), None);
    }
}
