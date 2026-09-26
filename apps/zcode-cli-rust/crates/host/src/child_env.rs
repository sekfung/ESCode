//! 子进程环境（docs/specs/rust-browser-use.md「第 4 期细则」）：Rust 不改写自身进程环境（多线程下
//! `set_var` 不安全），每次派生子进程时按启动环境计算差量；规则在 `domain::runtime_env`。
use zcode_cli_domain::runtime_env::{self, CuaCredentials};

fn vars() -> Vec<(String, String)> {
    // 非 UTF-8 的键值不可能命中清洗清单，原样继承即可。
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

/// `tool`：Bash、自定义命令与 MCP stdio（清洗后恢复出网配置）；否则只清洗（git、PDF 渲染等内部子进程）。
pub fn apply(command: &mut tokio::process::Command, tool: bool) {
    for (key, value) in runtime_env::child_env(&vars(), tool, cfg!(windows)) {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
}

/// 只供 node_repl 定向注入；其他子进程的环境里这些键已被 `apply` 删除。
pub fn cua_credentials() -> Option<CuaCredentials> {
    runtime_env::cua_credentials(&vars())
}
