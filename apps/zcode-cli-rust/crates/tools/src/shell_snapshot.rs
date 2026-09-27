//! Bash 的 shell 初始化快照与 cwd 捕获（docs/specs/rust-bash-shell-snapshot.md），对齐 TS
//! `ShellInitSnapshotManager`、`createCwdCapturePlan`/`readCapturedCwd` 与 `decideBashCwdPolicy`。
//! 文本规则在 `domain::shell_snapshot`；这里负责创建、缓存、清理与文件读写。
use crate::domain::shell_snapshot::{self as rules, ShellKind};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::OnceCell;

const TIMEOUT: Duration = Duration::from_secs(10);
const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Clone)]
pub(crate) struct Snapshot {
    path: PathBuf,
    shell_path: String,
}

impl Snapshot {
    /// TS `applyBashSourceScripts` 的首行：可选 source，失败不影响命令。
    pub(crate) fn source_line(&self) -> String {
        let native = self.path.to_string_lossy();
        let quoted = if native == self.shell_path {
            crate::embedded_search::quote_always(&self.shell_path)
        } else {
            crate::embedded_search::quote(&self.shell_path)
        };
        rules::optional_source_line(&quoted)
    }
}

type Cell = Arc<OnceCell<Option<Snapshot>>>;

fn cache() -> &'static Mutex<HashMap<String, Cell>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cell>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn created() -> &'static Mutex<Vec<PathBuf>> {
    static CREATED: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    CREATED.get_or_init(Default::default)
}

/// 按（根目录、方言、shell）取快照：进程内只创建一次；每次使用前确认文件仍在（TS `revalidate…`）。
pub(crate) async fn get(root: &Path, git_bash: bool, shell: &str, overlay: &[(String, String)]) -> Option<Snapshot> {
    let key = format!("{}:{}:{shell}", root.display(), if git_bash { "git-bash" } else { "posix" });
    let (cell, first) = {
        let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
        let first = cache.is_empty();
        (cache.entry(key).or_default().clone(), first)
    };
    if first {
        // TS 在 adapter 构造时清理 30 天前的快照；Rust 在首次使用时做一次，失败不影响执行。
        let root = root.to_owned();
        tokio::spawn(async move { cleanup_stale(&root).await });
    }
    let snapshot = cell.get_or_init(|| create(root, git_bash, shell, overlay)).await.clone()?;
    tokio::fs::metadata(&snapshot.path).await.ok()?;
    Some(snapshot)
}

async fn create(root: &Path, git_bash: bool, shell: &str, overlay: &[(String, String)]) -> Option<Snapshot> {
    let kind = ShellKind::detect(shell);
    let dir = root.join("shell-snapshots");
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_millis();
    let random: String = uuid::Uuid::new_v4().simple().to_string().chars().take(6).collect();
    let path = dir.join(format!("snapshot-{}-{millis}-{random}.sh", kind.as_str()));
    let native = path.to_string_lossy().into_owned();
    let shell_path = if git_bash { crate::embedded_search::windows_path_to_git_bash(&native) } else { native.clone() };
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let config = Path::new(&home).join(kind.config_file());
    let exists = tokio::fs::metadata(&config).await.is_ok();
    let path_value = match git_bash {
        true => run(shell, &["-lc", r#"echo "$PATH""#], overlay).await.map(|out| out.trim().to_owned()).filter(|p| !p.is_empty()),
        false => None,
    }
    .or_else(|| std::env::var("PATH").ok())
    .unwrap_or_default();
    let script = rules::creation_script(exists, &config.to_string_lossy(), &path_value, kind, &native);
    run(shell, &["-c", "-l", &script], overlay).await?;
    tokio::fs::metadata(&path).await.ok().filter(|m| m.is_file())?;
    created().lock().unwrap_or_else(|e| e.into_inner()).push(path.clone());
    Some(Snapshot { path, shell_path })
}

/// 以工具子进程环境运行 shell（TS 用同一份 snapshotEnv），10s 超时；成功时返回 stdout。
async fn run(shell: &str, args: &[&str], overlay: &[(String, String)]) -> Option<String> {
    let mut command = tokio::process::Command::new(shell);
    zcode_cli_host::child_env::apply(&mut command, true);
    command
        .args(args)
        .envs(overlay.iter().cloned())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(TIMEOUT, command.output()).await.ok()?.ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// runtime 关闭时删除本进程创建的快照（TS `cleanupRegistry`）。
pub(crate) async fn cleanup() {
    let paths = std::mem::take(&mut *created().lock().unwrap_or_else(|e| e.into_inner()));
    for path in paths {
        let _ = tokio::fs::remove_file(path).await;
    }
}

async fn cleanup_stale(root: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(root.join("shell-snapshots")).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let stale = entry.metadata().await.ok().filter(|m| m.is_file()).and_then(|m| m.modified().ok());
        if path.extension().is_some_and(|e| e == "sh")
            && stale.is_some_and(|t| t.elapsed().is_ok_and(|age| age > RETENTION))
        {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

/// cwd 捕获文件（TS 放在系统临时目录）。
pub(crate) fn cwd_file() -> PathBuf {
    std::env::temp_dir().join(format!("zcode-{}-cwd", uuid::Uuid::new_v4()))
}

/// TS `readCapturedCwd`：读后删除；git-bash 路径转回 Windows 路径；必须是目录，取 realpath。
pub(crate) async fn read_cwd(file: &Path, git_bash: bool) -> Option<PathBuf> {
    let text = tokio::fs::read_to_string(file).await;
    let _ = tokio::fs::remove_file(file).await;
    let text = text.ok()?;
    let value = text.strip_suffix('\n').map(|v| v.strip_suffix('\r').unwrap_or(v)).unwrap_or(&text);
    if value.is_empty() {
        return None;
    }
    let host = if git_bash { rules::git_bash_to_windows(value) } else { value.to_owned() };
    tokio::fs::metadata(&host).await.ok().filter(|m| m.is_dir())?;
    zcode_cli_host::realpath(&host).await.ok()
}

/// TS `decideBashCwdPolicy` 的边界判断：按物理路径比较是否仍在工作区内（Windows 不区分大小写）。
pub(crate) async fn outside(resolved: &Path, workspace: &Path) -> bool {
    let root = zcode_cli_host::realpath(workspace).await.unwrap_or_else(|_| workspace.to_owned());
    let fold = |p: &Path| {
        let text = p.to_string_lossy().trim_end_matches(['/', '\\']).to_owned();
        if cfg!(windows) { text.to_lowercase() } else { text }
    };
    let (resolved, root) = (fold(resolved), fold(&root));
    let inside = resolved == root
        || resolved.strip_prefix(&root).is_some_and(|rest| rest.starts_with(['/', '\\']));
    !inside
}
