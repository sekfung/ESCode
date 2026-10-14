//! 特权步骤（`--install-privileges`，docs/specs/usb-agent-tools.md §4.1）。
//!
//! 只做固定的四件事，**不接受任何路径或用户名参数**：目标用户从 `PKEXEC_UID`（pkexec）或
//! `SUDO_UID`（sudo）推导，避免把提权入口变成参数化的任意操作：
//!   1. 组 `escode-usb` 不存在则 `groupadd --system escode-usb`；把目标用户加入（`usermod -aG`）。
//!   2. 写 `/etc/udev/rules.d/99-escode-usb.rules`，可用则 `udevadm control --reload-rules`。
//!   3. `modprobe usbmon`（失败不致命）；写 `/etc/modules-load.d/escode-usb.conf`。
//!   4. 修正当前已存在的 `/dev/usbmon*` 属组与权限，让当前会话立即生效（不必重新登录）。
//!
//! 全过程幂等：已完成的步骤重复执行要成功而不是报错。

use std::fs;
use std::os::unix::fs::{PermissionsExt, chown};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::error::{ErrorCode, HelperError};

pub const GROUP_NAME: &str = "escode-usb";
pub const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/99-escode-usb.rules";
/// 只给「指定用户可读」：内核在 open/ioctl/mmap 上没有 capability 检查，最小授权就是让节点可读。
pub const UDEV_RULE: &str = "SUBSYSTEM==\"usbmon\", MODE=\"0640\", GROUP=\"escode-usb\"";
pub const MODULES_LOAD_PATH: &str = "/etc/modules-load.d/escode-usb.conf";
pub const MODULE_NAME: &str = "usbmon";

const DEV_DIR: &str = "/dev";
const NODE_MODE: u32 = 0o640;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    pub group: String,
    pub udev_rule: String,
    pub module: String,
    /// 已成功修正属组与权限的设备节点（路径）。
    pub nodes: Vec<String>,
}

impl InstallResult {
    pub fn to_json(&self) -> Value {
        json!({
            "ok": true,
            "group": self.group,
            "udevRule": self.udev_rule,
            "module": self.module,
            "nodes": self.nodes,
        })
    }
}

pub fn install_privileges() -> Result<InstallResult, HelperError> {
    // SAFETY: geteuid 无参数、无副作用。
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        return Err(HelperError::new(
            ErrorCode::Permission,
            format!("需要 root 权限（当前 euid = {euid}）。"),
        )
        .with_remediation(
            "通过 `pkexec escode-usb-capture --install-privileges` 或 `sudo` 运行；\
                     命令行不接受目标用户参数，用户身份从 PKEXEC_UID / SUDO_UID 推导。",
        ));
    }
    let uid = target_uid()?;
    let user = user_name(uid)?;
    let mut warnings: Vec<String> = Vec::new();

    // 1. 组 + 成员。
    let gid = ensure_group(&user, &mut warnings)?;

    // 2. udev 规则（覆盖将来创建的节点）。
    write_if_changed(Path::new(UDEV_RULE_PATH), &format!("{UDEV_RULE}\n"))?;
    reload_udev_rules(&mut warnings);

    // 3. 模块：立即加载 + 开机加载。
    load_module(&mut warnings);
    if let Some(parent) = Path::new(MODULES_LOAD_PATH).parent() {
        fs::create_dir_all(parent).map_err(|error| {
            HelperError::io(format!("创建目录 {} 失败：{error}", parent.display()))
        })?;
    }
    write_if_changed(Path::new(MODULES_LOAD_PATH), &format!("{MODULE_NAME}\n"))?;

    // 4. 当前已存在的节点立即生效（否则要等用户重新登录 / 重新插拔）。
    let nodes = fix_current_nodes(gid, &mut warnings);

    for warning in &warnings {
        eprintln!("警告：{warning}");
    }
    Ok(InstallResult {
        group: GROUP_NAME.to_string(),
        udev_rule: UDEV_RULE_PATH.to_string(),
        module: MODULE_NAME.to_string(),
        nodes,
    })
}

/// 目标用户：pkexec 给 `PKEXEC_UID`，sudo 给 `SUDO_UID`；都没有就报错，不接受命令行参数。
fn target_uid() -> Result<u32, HelperError> {
    for variable in ["PKEXEC_UID", "SUDO_UID"] {
        let Ok(raw) = std::env::var(variable) else {
            continue;
        };
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(uid) = raw.parse::<u32>() else {
            return Err(HelperError::invalid_input(format!(
                "{variable} 不是合法 UID：{raw}"
            )));
        };
        return Ok(uid);
    }
    Err(HelperError::new(
        ErrorCode::InvalidInput,
        "找不到 PKEXEC_UID / SUDO_UID，无法确定要授权的用户。",
    )
    .with_remediation(
        "通过 pkexec 或 sudo 运行本命令（直接以 root 登录时无法推导发起安装的用户）。",
    ))
}

/// UID → 用户名：先查 /etc/passwd（不依赖外部命令），查不到再退回 `id -nu`（NSS/LDAP 用户）。
fn user_name(uid: u32) -> Result<String, HelperError> {
    if let Ok(content) = fs::read_to_string("/etc/passwd") {
        for line in content.lines() {
            let mut fields = line.split(':');
            let name = fields.next().unwrap_or_default();
            let _password = fields.next();
            if fields.next().and_then(|value| value.parse::<u32>().ok()) == Some(uid)
                && safe_user_name(name)
            {
                return Ok(name.to_string());
            }
        }
    }
    if let Some(id) = find_tool("id") {
        let output = Command::new(&id)
            .args(["-nu", &uid.to_string()])
            .output()
            .map_err(|error| {
                HelperError::io(format!("执行 {} -nu {uid} 失败：{error}", id.display()))
            })?;
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if safe_user_name(&name) {
                return Ok(name);
            }
        }
    }
    Err(HelperError::io(format!(
        "无法解析 UID {uid} 对应的用户名（/etc/passwd 与 `id -nu` 都没有结果）。"
    )))
}

/// 用户名会作为 `usermod` 的一个 argv 传入；除 shell 注入外还要挡住以 `-` 开头（会被当成 flag）。
fn safe_user_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// 确保组存在并包含目标用户，返回 gid。uid 0（root）不需要也不加入该组。
fn ensure_group(user: &str, warnings: &mut Vec<String>) -> Result<u32, HelperError> {
    if let Some(group) = group_entry(GROUP_NAME) {
        if group.gid == 0 {
            return Err(HelperError::io(format!(
                "组 {GROUP_NAME} 的 gid 为 0，拒绝使用。"
            )));
        }
        if is_root_user(user) {
            warnings.push(format!(
                "目标用户 {user} 是 root，跳过加入 {GROUP_NAME} 组。"
            ));
        } else if !group.members.iter().any(|member| member == user) {
            run_tool_required("usermod", &["-aG", GROUP_NAME, user])?;
        }
        return Ok(group.gid);
    }
    run_tool_required("groupadd", &["--system", GROUP_NAME])?;
    let group = group_entry(GROUP_NAME).ok_or_else(|| {
        HelperError::io(format!(
            "groupadd 之后仍在 /etc/group 里找不到组 {GROUP_NAME}。"
        ))
    })?;
    if is_root_user(user) {
        warnings.push(format!(
            "目标用户 {user} 是 root，跳过加入 {GROUP_NAME} 组。"
        ));
    } else {
        run_tool_required("usermod", &["-aG", GROUP_NAME, user])?;
    }
    Ok(group.gid)
}

fn is_root_user(user: &str) -> bool {
    user == "root"
}

struct GroupEntry {
    gid: u32,
    members: Vec<String>,
}

/// 解析 /etc/group（`name:passwd:gid:member1,member2`）；只看本地组，NSS 组不在范围内。
fn group_entry(name: &str) -> Option<GroupEntry> {
    let content = fs::read_to_string("/etc/group").ok()?;
    for line in content.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 4 || fields[0] != name {
            continue;
        }
        let gid = fields[2].parse::<u32>().ok()?;
        let members = fields[3]
            .split(',')
            .map(str::trim)
            .filter(|member| !member.is_empty())
            .map(str::to_string)
            .collect();
        return Some(GroupEntry { gid, members });
    }
    None
}

fn reload_udev_rules(warnings: &mut Vec<String>) {
    let Some(udevadm) = find_tool("udevadm") else {
        warnings.push(
            "找不到 udevadm，跳过 `udevadm control --reload-rules`（规则文件已就位）。".to_string(),
        );
        return;
    };
    match run_tool(&udevadm, &["control", "--reload-rules"]) {
        Ok(()) => {}
        Err(error) => warnings.push(format!(
            "udevadm 重载规则失败：{error}（规则文件已就位，重新插拔设备或重启后生效）"
        )),
    }
}

/// modprobe 失败不致命：模块可能已经加载、可能编进了内核，也可能要等重启才生效。
fn load_module(warnings: &mut Vec<String>) {
    let Some(modprobe) = find_tool("modprobe") else {
        warnings
            .push("找不到 modprobe，跳过 `modprobe usbmon`（已写入 modules-load.d）。".to_string());
        return;
    };
    if let Err(error) = run_tool(&modprobe, &[MODULE_NAME]) {
        warnings.push(format!("modprobe usbmon 失败：{error}"));
    }
}

/// 修正当前已存在的节点：`chgrp escode-usb` + `chmod 640` 的等价 syscall 版本。
/// 用 chown/chmod 而不是外部命令，是因为 pkexec 环境的 PATH 可能极简，而且这里没有解析需求。
fn fix_current_nodes(gid: u32, warnings: &mut Vec<String>) -> Vec<String> {
    let mut fixed = Vec::new();
    let Ok(entries) = fs::read_dir(DEV_DIR) else {
        warnings.push(format!(
            "无法列出 {DEV_DIR}，没有修正任何 usbmon 设备节点。"
        ));
        return fixed;
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(digits) = name.strip_prefix("usbmon") else {
            continue;
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let display = path.display().to_string();
        if let Err(error) = chown(&path, None, Some(gid)) {
            warnings.push(format!("chgrp {GROUP_NAME} {display} 失败：{error}"));
            continue;
        }
        if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(NODE_MODE)) {
            warnings.push(format!("chmod {NODE_MODE:o} {display} 失败：{error}"));
            continue;
        }
        fixed.push(display);
    }
    fixed
}

/// 内容相同就不重写：保持幂等，也不无谓地改动 /etc 下文件的时间戳。
fn write_if_changed(path: &Path, content: &str) -> Result<(), HelperError> {
    if fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    fs::write(path, content)
        .map_err(|error| HelperError::io(format!("写入 {} 失败：{error}", path.display())))
}

/// 按 PATH + 常见 sbin 目录查找可执行文件；pkexec 下 PATH 可能不含 sbin。
fn find_tool(name: &str) -> Option<PathBuf> {
    const EXTRA_DIRS: [&str; 6] = [
        "/usr/sbin",
        "/sbin",
        "/usr/bin",
        "/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
    ];
    let mut directories: Vec<PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        directories.extend(
            path.split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        );
    }
    directories.extend(EXTRA_DIRS.iter().map(PathBuf::from));
    for directory in directories {
        let candidate = directory.join(name);
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

fn run_tool(program: &Path, args: &[&str]) -> Result<(), HelperError> {
    // 不经 shell：参数以 argv 传入，不存在解释器与命令拼接。
    let output = Command::new(program).args(args).output().map_err(|error| {
        HelperError::io(format!(
            "执行 {} {} 失败：{error}",
            program.display(),
            args.join(" ")
        ))
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(HelperError::io(format!(
        "{} {} 退出码 {:?}：{}",
        program.display(),
        args.join(" "),
        output.status.code(),
        if stderr.is_empty() {
            "（无 stderr）".to_string()
        } else {
            stderr
        }
    )))
}

/// 固定步骤依赖的系统工具不存在时是硬错误（不是「静默跳过」）：授权没做成就得如实失败。
fn run_tool_required(name: &str, args: &[&str]) -> Result<(), HelperError> {
    let Some(program) = find_tool(name) else {
        return Err(HelperError::io(format!(
            "找不到必要的系统工具 {name}（PATH 与 /usr/sbin 等常见目录都没有）。"
        )));
    };
    run_tool(&program, args)
}
