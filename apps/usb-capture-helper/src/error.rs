//! 错误码与 CLI 错误 JSON。错误码集合与 docs/specs/usb-agent-tools.md §4.1 一致，
//! 由插件映射成工具错误码；退出码是给直接调用方（脚本）用的冗余信息。

use std::fmt;
use std::io;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// 能看到设备节点但打不开（EACCES/EPERM），或需要 root 的特权步骤没在 root 下运行。
    Permission,
    /// 总线号没有对应的 usbmon 设备节点（模块未加载 / 总线号写错）。
    NoInterface,
    /// 设备已被别的读取者占用（usbmon 支持多读者，这条主要用于 Windows 服务语义的对齐）。
    Busy,
    /// 参数与路径校验失败：本工具只接受固定子命令与受限参数。
    InvalidInput,
    /// 其余 I/O 失败（打开、读写、改名、内核 ioctl 返回负值等）。
    Io,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::NoInterface => "no-interface",
            Self::Busy => "busy",
            Self::InvalidInput => "invalidInput",
            Self::Io => "io",
        }
    }

    pub fn exit_code(self) -> u8 {
        match self {
            Self::InvalidInput => 2,
            Self::Permission => 3,
            Self::NoInterface => 4,
            Self::Busy => 5,
            Self::Io => 6,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HelperError {
    pub code: ErrorCode,
    pub message: String,
    /// 人类可读的下一步；由插件原样转述给用户，不在这里做提权尝试。
    pub remediation: Option<String>,
}

impl HelperError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), remediation: None }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Io, message)
    }

    pub fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }

    /// `{ code, message, remediation? }`。插件把它包进 `{ ok: false, error: ... }`。
    pub fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("code".to_string(), Value::String(self.code.as_str().to_string()));
        map.insert("message".to_string(), Value::String(self.message.clone()));
        if let Some(remediation) = &self.remediation {
            map.insert("remediation".to_string(), Value::String(remediation.clone()));
        }
        Value::Object(map)
    }
}

impl fmt::Display for HelperError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for HelperError {}

/// 包一层 errno：抓包失败最常见的三类原因是权限、节点不存在、被占用，消息里带上原始 errno 便于用户判断。
pub fn io_context(context: impl AsRef<str>, error: &io::Error) -> String {
    format!("{}：{}（errno {:?}）", context.as_ref(), error, error.raw_os_error())
}

/// 打开 `/dev/usbmonN` 失败时的错误分类。
pub fn usbmon_open_error(bus: u32, error: &io::Error) -> HelperError {
    let path = format!("/dev/usbmon{bus}");
    let message = io_context(format!("无法打开 {path}"), error);
    match error.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => HelperError::new(ErrorCode::Permission, message)
            .with_remediation(
                "当前用户没有读取 usbmon 设备节点的权限：运行 `pkexec escode-usb-capture --install-privileges`\
                 （创建 escode-usb 组、写 udev 规则并修正现有节点），或 `sudo usermod -aG escode-usb $USER` 后重新登录。",
            ),
        Some(libc::ENOENT) | Some(libc::ENODEV) | Some(libc::ENXIO) => {
            HelperError::new(ErrorCode::NoInterface, message).with_remediation(
                "确认 usbmon 已加载（`sudo modprobe usbmon`）且总线号正确；用 `--probe` 查看可用接口。",
            )
        }
        Some(libc::EBUSY) => HelperError::new(ErrorCode::Busy, message)
            .with_remediation("稍后重试；同一条总线上另一个抓包作业可能还在收尾。"),
        _ => HelperError::io(message),
    }
}

/// ioctl / mmap 失败时的错误分类（`context` 说明是哪一步）。
pub fn syscall_error(context: impl AsRef<str>, error: &io::Error) -> HelperError {
    let message = io_context(context, error);
    match error.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => HelperError::new(ErrorCode::Permission, message),
        Some(libc::EBUSY) => HelperError::new(ErrorCode::Busy, message),
        _ => HelperError::io(message),
    }
}
