//! 显式环境快照与实例上下文（docs/specs/rust-u1-runtime-lib.md §3.1）。
//!
//! 本模块是整个 workspace 唯一允许读取 `std::env` 的地方（边界脚本唯一豁免，见
//! `scripts/check-escode-cli-rust-boundaries.mjs`）。crates/ 内一律通过这里的访问器
//! 取进程事实；快照由组合根（`src/main.rs` 的进程级安装、`src/embed.rs` 的实例级
//! RuntimeOptions）传入，嵌入实例与进程 env/cwd 不一致时以 RuntimeOptions 为准。

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

use crate::model_call::ModelUsageSink;

/// 一次运行可见的进程事实：环境变量、cwd、可执行文件与 argv。
/// `vars` 之外 Windows 另存一份小写键索引，复刻 `std::env::var` 在 Windows 上的
/// 大小写不敏感查找语义（TS/Node 同样如此）。
pub struct EnvSnapshot {
    vars: HashMap<OsString, OsString>,
    #[cfg(windows)]
    lower: HashMap<OsString, OsString>,
    cwd: PathBuf,
    exe: PathBuf,
    argv: Vec<String>,
}

impl EnvSnapshot {
    /// 组合根用：采集真实进程事实（binary main 安装进程级快照）。
    pub fn capture_process() -> Self {
        Self::from_parts(
            std::env::vars_os(),
            std::env::current_dir().unwrap_or_default(),
            std::env::args().collect(),
            std::env::current_exe().unwrap_or_default(),
        )
    }

    /// 嵌入入口用：RuntimeOptions 给出的实例事实（`argv` 含 argv[0]，与 `std::env::args` 同形）。
    pub fn from_parts(
        vars: impl IntoIterator<Item = (OsString, OsString)>,
        cwd: PathBuf,
        argv: Vec<String>,
        exe: PathBuf,
    ) -> Self {
        let snapshot = Self {
            vars: HashMap::from_iter(vars),
            #[cfg(windows)]
            lower: HashMap::new(),
            cwd,
            exe,
            argv,
        };
        #[cfg(windows)]
        let snapshot = {
            let mut snapshot = snapshot;
            for (key, value) in &snapshot.vars {
                if let Some(name) = lower_key(key) {
                    snapshot.lower.insert(name, value.clone());
                }
            }
            snapshot
        };
        snapshot
    }

    pub fn get(&self, key: impl AsRef<OsStr>) -> Option<&OsString> {
        let key = key.as_ref();
        #[cfg(windows)]
        {
            if self.vars.contains_key(key) {
                return self.vars.get(key);
            }
            lower_key(key).and_then(|name| self.lower.get(&name))
        }
        #[cfg(not(windows))]
        self.vars.get(key)
    }

    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    pub fn exe(&self) -> &std::path::Path {
        &self.exe
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// 有序（按变量名）确定性行迭代；`std::env::vars_os` 的顺序本就不由任何消费方依赖。
    pub fn vars_os(&self) -> Vec<(OsString, OsString)> {
        let mut pairs: Vec<_> = self
            .vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }
}

#[cfg(windows)]
fn lower_key(key: &OsStr) -> Option<OsString> {
    key.to_str().map(|name| OsString::from(name.to_lowercase()))
}

/// 一个嵌入运行时实例的横切状态：env 快照、任务作用域与实例级端口覆盖。
/// 详见 docs/specs/rust-u1-runtime-lib.md §3.2（实例级全局状态归属）。
pub struct InstanceContext {
    pub env: Arc<EnvSnapshot>,
    pub spawner: Weak<crate::scope::Spawner>,
    /// 实例级用量落点覆盖：`set_model_usage_sink` 在实例树内调用时写这里（多实例不串扰）。
    pub usage_sink: OnceLock<ModelUsageSink>,
    /// 实例级 TS 派生媒体缓存根覆盖（`set_media_cache_root` 同上）。
    pub media_cache_root: OnceLock<PathBuf>,
}

tokio::task_local! {
    pub(crate) static INSTANCE: Arc<InstanceContext>;
}

/// 进程级快照（binary main 在启动时安装一次；先到先得）。
static PROCESS: OnceLock<Arc<EnvSnapshot>> = OnceLock::new();
static PROCESS_USAGE_SINK: OnceLock<ModelUsageSink> = OnceLock::new();
static PROCESS_MEDIA_CACHE_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// binary main 组合根安装进程级快照；此后所有任务（含嵌入前的早期路径）以它兜底。
pub fn install_process_snapshot() {
    let _ = PROCESS.set(Arc::new(EnvSnapshot::capture_process()));
}

/// 当前生效的 env 快照：实例 task-local 优先，其次进程级安装，缺席返回 None。
pub fn current_snapshot() -> Option<Arc<EnvSnapshot>> {
    INSTANCE
        .try_with(|context| context.env.clone())
        .ok()
        .or_else(|| PROCESS.get().cloned())
}

/// 当前实例上下文（实例树内才有；进程级装配路径返回 None）。
pub fn current_instance() -> Option<Arc<InstanceContext>> {
    INSTANCE.try_with(Clone::clone).ok()
}

/// 在实例上下文下执行一个未来（embed 实例根任务用：装配与 serve 的整个子树都
/// 以 RuntimeOptions 的 env/端口覆盖解析进程事实）。子任务经 Spawner 重建上下文。
pub async fn run_in_instance<F: std::future::Future>(
    context: Arc<InstanceContext>,
    fut: F,
) -> F::Output {
    INSTANCE.scope(context, fut).await
}

pub fn var(name: &str) -> Result<String, std::env::VarError> {
    let value = match current_snapshot() {
        Some(snapshot) => snapshot.get(name).cloned(),
        None => std::env::var_os(name),
    };
    match value {
        Some(value) => value.into_string().map_err(std::env::VarError::NotUnicode),
        None => Err(std::env::VarError::NotPresent),
    }
}

pub fn var_os(name: &str) -> Option<OsString> {
    match current_snapshot() {
        Some(snapshot) => snapshot.get(name).cloned(),
        None => std::env::var_os(name),
    }
}

/// 全量环境（名字排序的确定性顺序；调用方多为一次性遍历）。
pub fn vars() -> Vec<(String, String)> {
    vars_os()
        .into_iter()
        .map(|(key, value)| {
            let name = key
                .into_string()
                .expect("environment variable name is valid Unicode");
            let value = value
                .into_string()
                .expect("environment variable is valid Unicode");
            (name, value)
        })
        .collect()
}

pub fn vars_os() -> Vec<(OsString, OsString)> {
    match current_snapshot() {
        Some(snapshot) => snapshot.vars_os(),
        None => std::env::vars_os().collect(),
    }
}

pub fn current_dir() -> io::Result<PathBuf> {
    match current_snapshot() {
        Some(snapshot) => Ok(snapshot.cwd().to_path_buf()),
        None => std::env::current_dir(),
    }
}

pub fn current_exe() -> io::Result<PathBuf> {
    match current_snapshot() {
        Some(snapshot) => Ok(snapshot.exe().to_path_buf()),
        None => std::env::current_exe(),
    }
}

pub fn args() -> std::vec::IntoIter<String> {
    match current_snapshot() {
        Some(snapshot) => snapshot.argv().to_vec().into_iter(),
        None => std::env::args().collect::<Vec<_>>().into_iter(),
    }
}

/// `TMPDIR` 按进程语义（不随实例隔离，已登记为已批准差异，rust-u1-runtime-lib.md §3.2）。
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

/// 用量落点装配：实例树内调用写实例槽（先到先得，与进程级 OnceLock 语义一致），否则写进程槽。
pub fn set_model_usage_sink(sink: ModelUsageSink) {
    // ModelUsageSink 不可克隆：先装实例槽，装不上时取回再落进程槽。
    let mut pending = Some(sink);
    let installed = INSTANCE
        .try_with(|context| {
            let value = pending.take().expect("sink present on first try_with");
            context.usage_sink.set(value).is_ok()
        })
        .unwrap_or(false);
    if !installed && let Some(value) = pending {
        let _ = PROCESS_USAGE_SINK.set(value);
    }
}

pub fn set_media_cache_root(root: PathBuf) {
    let installed = INSTANCE
        .try_with(|context| context.media_cache_root.set(root.clone()).is_ok())
        .unwrap_or(false);
    if !installed {
        let _ = PROCESS_MEDIA_CACHE_ROOT.set(root);
    }
}

pub fn media_cache_root() -> Option<PathBuf> {
    let instance = INSTANCE
        .try_with(|context| context.media_cache_root.get().cloned())
        .ok()
        .flatten();
    instance.or_else(|| PROCESS_MEDIA_CACHE_ROOT.get().cloned())
}

/// 在生效的用量落点上记录一条事实（实例槽优先；task-local 借用不能逃出 try_with，
/// 所以在闭包内调用）。返回是否已投递实例槽，供回退判断。
pub(crate) fn record_to_instance_sink(fact: serde_json::Value) -> bool {
    INSTANCE
        .try_with(|context| match context.usage_sink.get() {
            Some(sink) => {
                sink(fact);
                true
            }
            None => false,
        })
        .unwrap_or(false)
}

/// 进程级用量槽（env 模块私有状态的 crate 内只读出口）。
pub(crate) fn process_usage_sink() -> Option<&'static ModelUsageSink> {
    PROCESS_USAGE_SINK.get()
}
