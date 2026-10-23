//! 内置 `rea` MCP（docs/specs/rea-plugin.md），对齐 TS `resolveBuiltInReaMcpServers`：
//! REA 是第三方逆向分析工具（morluto/rea，MIT），运行时随应用分发在 agent bundle 的 `rea-runtime/`，
//! 经 Host 插件启动器用自带 Node 运行插件的 `dist/mcp/server.js` —— 因此不要求用户装 Node/npm，
//! 也绕开 Rust 宿主无法执行 `npx` 这类 `.cmd` shim 的限制。
//!
//! 与 serial 的两点不同：没有 broker，所以没有 `ESCODE_HOST_*` 门禁（门禁是「插件启用 + 启动器
//! env 可用」）；隔离语义是 `session` 而不是 `workspace`（REA 在服务端持有 binary session）。
use super::{extension_plugins::Plugin, mcp_config::Server};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const NAME: &str = "rea";
const PLUGIN: &str = "rea@zcode-plugins-official";
/// 90s：Ghidra/Hopper 启动、JADX 反编译与浏览器抓取都偏慢，默认 30s 不够（TS REA_MCP_TIMEOUT_MS 同）。
const TIMEOUT_MS: u64 = 90_000;
/// 逃生口选项：不映射成同名 env，而是把 JSON 对象的键值直接并入 env（上游的 REA_* 开关还在增加）。
const EXTRA_ENV_OPTION: &str = "extraEnvJson";
/// 免审批名单不在这里手抄。REA 有 170 个工具、其中 113 个属"读语义"（其余是写、执行、运行时附加、
/// 驱动界面、网络与会话启动类），上百个名字在 TS 与 Rust 各写一份必然漂移。流程是：
/// scripts/generate-rea-auto-approved-tools.mjs 从已安装的 REA 生成 TS 清单（带版本）→
/// 定义进 official-plugin-definitions.ts → scripts/generate-escode-cli-rust-official-plugins.mjs
/// 随 official_plugins.json 下发到这里；bootstrap/tests/rea-plugin.test.ts 保证清单与 pin 版本一致。
fn auto_approved_tool_names() -> Vec<String> {
    super::official_plugins::assets()
        .definitions
        .iter()
        .find(|definition| definition.name == NAME)
        .map(|definition| definition.default_allowed_tools.clone())
        .unwrap_or_default()
}

/// 无 broker 形态的门禁：Host 提供了插件启动器才可能注册（Node 运行时同判据）。
/// 判据抽成纯函数，测试直接给值 —— 测试里改进程环境在 Rust 2024 是 unsafe，且会污染全局状态。
pub fn launcher_env_available(exec: Option<&str>, entrypoint: Option<&str>) -> bool {
    [exec, entrypoint]
        .into_iter()
        .all(|value| value.is_some_and(|value| !value.trim().is_empty()))
}

pub fn host_rea_available() -> bool {
    launcher_env_available(
        escode_cli_core_api::env::var("ESCODE_PLUGIN_HOST_EXEC_PATH")
            .ok()
            .as_deref(),
        escode_cli_core_api::env::var("ESCODE_PLUGIN_HOST_ENTRYPOINT")
            .ok()
            .as_deref(),
    )
}

/// manifest 选项键 → REA 的 env 名。不做对照表：按 SCREAMING_SNAKE 机械转换
/// （ghidraInstallDir → GHIDRA_INSTALL_DIR、reaAnalysisProvider → REA_ANALYSIS_PROVIDER），
/// 与 TS `optionKeyToEnvName` 同规则 —— 两边各写一张表必然会漂移。
fn option_key_to_env_name(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for character in key.chars() {
        if character.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(character.to_ascii_uppercase());
    }
    out
}

fn option_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// 可选增强（Ghidra/Hopper/IDA/JDK/JADX/WABT/tshark/JEB/ILSpy…）刻意不随包：商业引擎无法分发，
/// 其余是平台与体积问题。用户填的路径优先，其次取 manifest 的 userConfig 默认值（TS 同），
/// 未填不注入 —— 交给 REA 自己探测与报错。
fn engine_env(plugin: &Plugin, options: &Value) -> Vec<(String, String)> {
    let defaults = &plugin.manifest["userConfig"];
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for map in [options.as_object(), defaults.as_object()]
        .into_iter()
        .flatten()
    {
        keys.extend(map.keys().cloned());
    }

    let mut env = Vec::new();
    for key in keys {
        if key == EXTRA_ENV_OPTION {
            continue;
        }
        let configured = options.get(key.as_str()).and_then(option_text);
        let fallback = defaults
            .get(key.as_str())
            .and_then(|entry| option_text(&entry["default"]));
        if let Some(value) = configured.or(fallback) {
            env.push((option_key_to_env_name(&key), value));
        }
    }
    if let Some(Value::String(raw)) = options.get(EXTRA_ENV_OPTION) {
        // 非法 JSON 只让这一项失效，不阻断 server 启动（表单项说明已写明必须是 JSON 对象）。
        if let Ok(Value::Object(extra)) = serde_json::from_str::<Value>(raw.trim()) {
            for (key, value) in extra {
                if let Some(value) = option_text(&value) {
                    env.push((key, value));
                }
            }
        }
    }
    env
}

/// 运行时解析顺序与 TS `resolveReaRuntimeRoot` 必须一致：agent bundle 旁的 rea-runtime（打包/远端），
/// 否则插件根 node_modules（开发态：插件直接从仓库目录解析）。
fn resolve_runtime_root(plugin_root: &Path) -> Option<PathBuf> {
    let entry_relative = ["rea-agents", "scripts", "rea.mjs"];
    let mut candidates = Vec::new();
    if let Ok(entrypoint) = escode_cli_core_api::env::var("ESCODE_PLUGIN_HOST_ENTRYPOINT")
        && let Some(directory) = Path::new(entrypoint.trim()).parent()
    {
        candidates.push(directory.join("rea-runtime").join("node_modules"));
    }
    candidates.push(plugin_root.join("node_modules"));
    candidates.into_iter().find(|candidate| {
        candidate
            .join(entry_relative[0])
            .join(entry_relative[1])
            .join(entry_relative[2])
            .is_file()
    })
}

fn rea_env(plugin: &Plugin, options: &Value) -> Vec<(String, String)> {
    let mut env = vec![(
        // 170 个工具的完整输入 schema 约 425 KB，compact 压到约 253 KB；部分 provider 会直接拒收大 schema。
        "REA_MCP_INPUT_SCHEMA_PROFILE".to_owned(),
        "compact".to_owned(),
    )];
    if let Some(root) = resolve_runtime_root(&plugin.root) {
        env.push((
            "REA_RUNTIME_ROOT".to_owned(),
            root.to_string_lossy().into_owned(),
        ));
    }
    env.extend(engine_env(plugin, options));
    env
}

pub fn server(
    plugins: &[Plugin],
    cwd: &Path,
    options_by_plugin: &Value,
    data_root: &Path,
) -> Option<Server> {
    if !host_rea_available() {
        return None;
    }
    let plugin = plugins.iter().find(|candidate| candidate.id == PLUGIN)?;
    let mut extra_env = rea_env(plugin, &options_by_plugin[PLUGIN]);
    // 诊断落盘目录（与 TS 侧 PluginMetadata.dataPath 同一算法）：入口把 REA 的 stderr tee 到这里，
    // 并写运行时自检报告 —— Rust 宿主丢弃子进程 stderr，打包态这是唯一的诊断入口。
    extra_env.push((
        "ESCODE_PLUGIN_DATA".to_owned(),
        data_root
            .join(escode_cli_domain::custom_command::sanitize_plugin_id(
                PLUGIN,
            ))
            .to_string_lossy()
            .into_owned(),
    ));
    // session 隔离：与 TS 侧默认一致（插件/host 注入以外没有 isolation 入口，默认即 session）。
    super::mcp_serial::host_plugin_server(
        NAME, PLUGIN, TIMEOUT_MS, "session", &extra_env, plugins, cwd,
    )
}

/// 并入权限 `allowedTools` 的免批准工具。插件关闭时这些工具不存在，放行项不生效。
pub fn default_allowed_tools(available: bool) -> Vec<String> {
    if !available {
        return vec![];
    }
    auto_approved_tool_names()
        .into_iter()
        .map(|tool| format!("mcp__{NAME}__{tool}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_both_launcher_env_values() {
        assert!(!launcher_env_available(None, None));
        assert!(!launcher_env_available(Some("/bin/escode-helper"), None));
        assert!(!launcher_env_available(None, Some("/app/escode.cjs")));
        assert!(!launcher_env_available(Some("  "), Some("/app/escode.cjs")));
        assert!(launcher_env_available(
            Some("/bin/escode-helper"),
            Some("/app/escode.cjs")
        ));
    }

    #[test]
    fn auto_approved_tools_come_from_the_generated_asset() {
        let names = auto_approved_tool_names();
        // 生成物里必须有 rea 的 defaultAllowedTools；为空说明 TS 定义或生成器断了。
        assert!(
            !names.is_empty(),
            "generated defaultAllowedTools missing for rea"
        );
        assert!(names.contains(&"binary_session".to_owned()));
        assert!(names.contains(&"batch_decompile".to_owned()));
        // 破坏性与写/会话类必须逐次审批。
        for gated in [
            "close_binary",
            "export_evidence_bundle",
            "unset_bookmark",
            "open_binary",
            "set_comment",
            "install_adb_package",
            "capture_web_screenshot",
        ] {
            assert!(
                !names.iter().any(|name| name == gated),
                "{gated} must stay approval-gated"
            );
        }
        let allowed = default_allowed_tools(true);
        assert!(allowed.contains(&"mcp__rea__binary_session".to_owned()));
        assert!(default_allowed_tools(false).is_empty());
    }

    #[test]
    fn maps_option_keys_to_screaming_snake_env_names() {
        // 与 TS optionKeyToEnvName 同规则；改成手写对照表就会在两边漂移。
        assert_eq!(
            option_key_to_env_name("ghidraInstallDir"),
            "GHIDRA_INSTALL_DIR"
        );
        assert_eq!(
            option_key_to_env_name("reaAnalysisProvider"),
            "REA_ANALYSIS_PROVIDER"
        );
        assert_eq!(option_key_to_env_name("javaHome"), "JAVA_HOME");
    }

    fn plugin_with(manifest: Value) -> Plugin {
        Plugin {
            enabled: true,
            id: PLUGIN.to_owned(),
            manifest,
            marketplace: "zcode-plugins-official".to_owned(),
            name: "rea".to_owned(),
            official: true,
            root: PathBuf::from("/plugins/rea-plugin"),
            source: "builtin",
        }
    }

    #[test]
    fn engine_env_prefers_configured_options_then_manifest_defaults() {
        let plugin = plugin_with(serde_json::json!({
            "userConfig": {
                "hopperLauncherPath": { "default": "/Applications/Hopper Disassembler.app/Contents/MacOS/hopper" },
                "reaWabtBinDirectory": {}
            }
        }));
        let options = serde_json::json!({
            "extraEnvJson": "{\"REA_GHIDRA_LANGUAGE_ID\":\"x86:LE:64:default\",\"BLANK\":\"  \"}",
            "ghidraInstallDir": " /opt/ghidra ",
            "javaHome": "/usr/lib/jvm/temurin-21",
            "reaAnalysisProvider": "ghidra"
        });
        let env: std::collections::BTreeMap<String, String> =
            engine_env(&plugin, &options).into_iter().collect();

        assert_eq!(
            env.get("GHIDRA_INSTALL_DIR").map(String::as_str),
            Some("/opt/ghidra")
        );
        assert_eq!(
            env.get("JAVA_HOME").map(String::as_str),
            Some("/usr/lib/jvm/temurin-21")
        );
        assert_eq!(
            env.get("REA_ANALYSIS_PROVIDER").map(String::as_str),
            Some("ghidra")
        );
        // 用户未填但 manifest 有默认 → 按默认注入；既无值也无默认 → 不注入。
        assert!(
            env.get("HOPPER_LAUNCHER_PATH")
                .is_some_and(|value| value.contains("Hopper"))
        );
        assert!(!env.contains_key("REA_WABT_BIN_DIRECTORY"));
        // 逃生口：JSON 对象键值并入 env，空白值不注入，选项本身不映射。
        assert_eq!(
            env.get("REA_GHIDRA_LANGUAGE_ID").map(String::as_str),
            Some("x86:LE:64:default")
        );
        assert!(!env.contains_key("BLANK"));
        assert!(!env.contains_key("EXTRA_ENV_JSON"));
    }

    #[test]
    fn invalid_extra_env_json_is_ignored_without_failing() {
        let plugin = plugin_with(serde_json::json!({ "userConfig": {} }));
        let options = serde_json::json!({ "extraEnvJson": "{ not json" });
        assert!(engine_env(&plugin, &options).is_empty());
    }
}
