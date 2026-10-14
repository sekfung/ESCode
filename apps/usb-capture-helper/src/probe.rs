//! 能力探测与中文 remediation（docs/specs/usb-agent-tools.md §4.1 的 `--probe`）。
//!
//! 探测本身不需要任何特权：`/sys/module/usbmon` 是否存在（模块是否已加载）是文件系统问题；
//! 权限判定用「以只读方式试着打开第一个设备节点」，`EACCES/EPERM` 即 denied。
//! 插件不尝试提权，只把这里给出的 `remediation` 原样转述给用户。

use std::fs::{self, File};
use std::path::Path;

use serde_json::{Value, json};

/// udev 规则里使用的组名（与 `install_privileges` 共用）。
pub const GROUP_NAME: &str = "escode-usb";

/// usbmon 的设备节点目录：节点名 `usbmonN` 的 N 就是总线号。
const DEV_DIR: &str = "/dev";
const MODULE_SYSFS: &str = "/sys/module/usbmon";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    Ok,
    Denied,
}

impl Privilege {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInfo {
    /// 设备节点名，例如 `usbmon1`。
    pub id: String,
    pub bus: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    pub platform: &'static str,
    pub available: bool,
    pub usbmon_module_loaded: bool,
    pub privilege: Privilege,
    pub interfaces: Vec<InterfaceInfo>,
    pub reason: Option<String>,
    pub remediation: Option<String>,
}

impl ProbeReport {
    pub fn to_json(&self) -> Value {
        let interfaces: Vec<Value> = self
            .interfaces
            .iter()
            .map(|item| json!({ "id": item.id, "bus": item.bus }))
            .collect();
        let mut map = serde_json::Map::new();
        map.insert("ok".to_string(), Value::Bool(true));
        map.insert(
            "platform".to_string(),
            Value::String(self.platform.to_string()),
        );
        map.insert("available".to_string(), Value::Bool(self.available));
        map.insert(
            "usbmonModuleLoaded".to_string(),
            Value::Bool(self.usbmon_module_loaded),
        );
        map.insert(
            "privilege".to_string(),
            Value::String(self.privilege.as_str().to_string()),
        );
        map.insert("interfaces".to_string(), Value::Array(interfaces));
        if let Some(reason) = &self.reason {
            map.insert("reason".to_string(), Value::String(reason.clone()));
        }
        if let Some(remediation) = &self.remediation {
            map.insert(
                "remediation".to_string(),
                Value::String(remediation.clone()),
            );
        }
        Value::Object(map)
    }
}

/// 列出 `/dev/usbmon*`。`usbmon0` 是「所有总线」的聚合设备，不作为抓包目标（--bus 只接受 1..=255），
/// 因此这里不导出它，避免上层把它当成一条可选总线。
pub fn list_interfaces() -> Vec<InterfaceInfo> {
    let mut interfaces = Vec::new();
    let Ok(entries) = fs::read_dir(DEV_DIR) else {
        return interfaces;
    };
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
        let Ok(bus) = digits.parse::<u32>() else {
            continue;
        };
        if bus == 0 {
            continue;
        }
        interfaces.push(InterfaceInfo {
            id: name.to_string(),
            bus,
        });
    }
    interfaces.sort_by_key(|item| item.bus);
    interfaces
}

/// 以只读方式试着打开节点：能打开就是有读权限（内核在这条路径上没有 capability 检查）。
fn readable(interface: &InterfaceInfo) -> Result<(), std::io::Error> {
    File::open(Path::new(DEV_DIR).join(&interface.id)).map(|_file| ())
}

pub fn probe() -> ProbeReport {
    let platform = std::env::consts::OS;
    let module_sysfs = Path::new(MODULE_SYSFS).exists();
    let interfaces = if platform == "linux" {
        list_interfaces()
    } else {
        Vec::new()
    };
    // usbmon 也可能编进内核（此时没有 /sys/module/usbmon 但有设备节点）：以节点为准，避免自相矛盾。
    let module_loaded = module_sysfs || !interfaces.is_empty();
    let (privilege, reason, remediation) =
        evaluate(platform, module_loaded, module_sysfs, &interfaces);
    ProbeReport {
        platform,
        available: privilege == Privilege::Ok,
        usbmon_module_loaded: module_loaded,
        privilege,
        interfaces,
        reason,
        remediation,
    }
}

/// 判定权限并给出「下一步做什么」的中文说明；三种不可用原因各自的修复动作不同，不能混用一句话。
fn evaluate(
    platform: &'static str,
    module_loaded: bool,
    module_sysfs: bool,
    interfaces: &[InterfaceInfo],
) -> (Privilege, Option<String>, Option<String>) {
    if platform != "linux" {
        return (
            Privilege::Denied,
            Some(format!(
                "本抓包器只支持 Linux usbmon，当前平台是 {platform}。"
            )),
            Some(
                "在 Linux 上使用内置抓包器；其它平台走 dumpcap 兼容后端或离线 pcapng 解码。"
                    .to_string(),
            ),
        );
    }
    if interfaces.is_empty() {
        let reason = if module_sysfs {
            "usbmon 已加载，但没有 /dev/usbmonN 设备节点。".to_string()
        } else if module_loaded {
            "没有 /dev/usbmonN 设备节点。".to_string()
        } else {
            "usbmon 内核模块未加载（没有 /sys/module/usbmon，也没有 /dev/usbmonN）。".to_string()
        };
        return (
            Privilege::Denied,
            Some(reason),
            Some(
                "加载模块后重试：`sudo modprobe usbmon`；\
                 也可以用 `pkexec escode-usb-capture --install-privileges` 一次性完成加载与授权。"
                    .to_string(),
            ),
        );
    }
    match readable(&interfaces[0]) {
        Ok(()) => (Privilege::Ok, None, None),
        Err(error) => {
            use std::io::ErrorKind;
            let denied = matches!(error.kind(), ErrorKind::PermissionDenied);
            let reason = if denied {
                format!(
                    "当前用户没有读取 /dev/{} 的权限（{error}）。",
                    interfaces[0].id
                )
            } else {
                format!("无法以只读方式打开 /dev/{}：{error}", interfaces[0].id)
            };
            (
                Privilege::Denied,
                Some(reason),
                Some(format!(
                    "运行 `pkexec escode-usb-capture --install-privileges`（创建 {GROUP_NAME} 组、\
                         写 udev 规则并修正现有节点属组），或 `sudo usermod -aG {GROUP_NAME} $USER` 后重新登录；\
                         也可以临时用 `sudo escode-usb-capture ...` 抓包。"
                )),
            )
        }
    }
}
