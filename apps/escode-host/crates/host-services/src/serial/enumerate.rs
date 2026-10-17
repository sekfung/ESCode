//! 串口枚举（S1）：`rust-u2-serial.md` §3。
//!
//! Linux 走 sysfs：`/sys/class/tty/<name>/device` 存在才是真实设备（虚拟 tty/pty 没有），
//! 端口路径是 `/dev/<name>`；元数据先读 `device/<field>`，再回退到 `device/../<field>`
//! （USB 串口的 `idVendor`/`idProduct`/`manufacturer`/`serial` 挂在 USB 设备节点上）。
//!
//! **登记差异**：非 Linux 平台本片返回空列表（TS 走 `bindings-cpp` 的平台后端）。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// 一个端口的枚举结果（`path` 必有，其余按设备实际存在的字段出现）。
pub type PortInfo = Map<String, Value>;

/// 端口枚举器：生产实现读 sysfs，测试注入假实现。
#[async_trait::async_trait]
pub trait PortEnumerator: Send + Sync {
    async fn list(&self) -> Result<Vec<PortInfo>, String>;
}

/// 生产实现：Linux sysfs。
pub struct SysfsEnumerator {
    sys_root: PathBuf,
    dev_root: PathBuf,
}

impl Default for SysfsEnumerator {
    fn default() -> Self {
        Self {
            sys_root: PathBuf::from("/sys/class/tty"),
            dev_root: PathBuf::from("/dev"),
        }
    }
}

impl SysfsEnumerator {
    /// 测试用：把 `/sys/class/tty` 与 `/dev` 换成夹具目录。
    pub fn with_roots(sys_root: PathBuf, dev_root: PathBuf) -> Self {
        Self { sys_root, dev_root }
    }
}

/// 读一个 sysfs 文本字段（trim 后非空才返回）。
fn read_field(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// `device/<field>` → `device/../<field>`（USB 串口的元数据在接口的上一级）。
fn read_metadata(device: &Path, field: &str) -> Option<String> {
    if let Some(value) = read_field(&device.join(field)) {
        return Some(value);
    }
    let parent = device.parent()?;
    read_field(&parent.join(field))
}

#[async_trait::async_trait]
impl PortEnumerator for SysfsEnumerator {
    async fn list(&self) -> Result<Vec<PortInfo>, String> {
        // 登记差异：非 Linux 本片返回空列表（TS 走 bindings-cpp 的平台后端）。
        if !cfg!(target_os = "linux") {
            return Ok(Vec::new());
        }
        let sys_root = self.sys_root.clone();
        let dev_root = self.dev_root.clone();
        tokio::task::spawn_blocking(move || enumerate_sync(&sys_root, &dev_root))
            .await
            .map_err(|error| format!("{error}"))?
    }
}

fn enumerate_sync(sys_root: &Path, dev_root: &Path) -> Result<Vec<PortInfo>, String> {
    let entries = std::fs::read_dir(sys_root).map_err(|error| format!("{error}"))?;
    let mut ports: Vec<PortInfo> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let device = entry.path().join("device");
        // 虚拟 tty（pty、`ttyprintk` 等）没有 `device` 子目录。
        if !device.is_dir() {
            continue;
        }
        let path = dev_root.join(&name);
        // `/dev` 里没有对应节点时同样不算一个可用端口。
        if !path.exists() {
            continue;
        }
        let mut info = Map::new();
        info.insert(
            "path".to_owned(),
            Value::from(path.to_string_lossy().into_owned()),
        );
        for (field, key) in [
            ("manufacturer", "manufacturer"),
            ("serial", "serialNumber"),
            ("idVendor", "vendorId"),
            ("idProduct", "productId"),
        ] {
            if let Some(value) = read_metadata(&device, field) {
                info.insert(key.to_owned(), Value::from(value));
            }
        }
        ports.push(info);
    }
    ports.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["path"].as_str().unwrap_or_default())
    });
    Ok(ports)
}

/// 端口列表的 dedupe 键：`ports.map(p => p.path).join("\n")`。
pub fn ports_key(ports: &[PortInfo]) -> String {
    ports
        .iter()
        .map(|port| port["path"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 空端口对象（`{"path": ...}` 的形状便于测试构造）。
pub fn port(path: &str) -> PortInfo {
    let mut info = Map::new();
    info.insert("path".to_owned(), Value::from(path));
    info
}

/// 便捷构造：带厂商/序列号等元数据的端口。
pub fn port_with(path: &str, fields: &[(&str, &str)]) -> PortInfo {
    let mut info = port(path);
    for (key, value) in fields {
        info.insert((*key).to_owned(), Value::from(*value));
    }
    info
}

#[cfg(test)]
#[path = "enumerate_tests.rs"]
mod tests;
