//! 串口传输层（S2）：`rust-u2-serial.md` §9。
//!
//! 生产实现是 Linux termios（`libc`）：`open(O_RDWR|O_NOCTTY|O_NONBLOCK)` + `tcgetattr`/`cfsetspeed`/
//! `tcsetattr`，关闭即 `close(fd)`。测试注入假的 `SerialTransport`（不需要真硬件）。
//!
//! **登记差异**：非标准波特率（TS 用 `termios2`/`BOTHER` 支持任意整数）在这里被拒绝为
//! `invalidConfig`；标准速率集覆盖了串口调试的实际用法（9600…921600）。

use serde_json::{Value, json};

/// `SerialConfig`（TS 同名接口）。
#[derive(Clone, Debug, PartialEq)]
pub struct SerialConfig {
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub rtscts: bool,
    pub auto_reconnect: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    None,
    Even,
    Odd,
    Mark,
    Space,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopBits {
    One,
    OneHalf,
    Two,
}

impl SerialConfig {
    /// 从 `SerialConfig` 的 JSON 形状解析并校验（`validateConfig`）。
    pub fn from_json(value: &Value) -> Result<Self, SerialOpenError> {
        let mut problems: Vec<&str> = Vec::new();
        let baud_rate = value["baudRate"].as_u64().unwrap_or(0);
        if baud_rate == 0 || baud_rate > u32::MAX as u64 {
            problems.push("baudRate");
        }
        let data_bits = value["dataBits"].as_u64().unwrap_or(0);
        let data_bits = match data_bits {
            5..=8 => data_bits as u8,
            _ => {
                problems.push("dataBits");
                8
            }
        };
        let parity = match value["parity"].as_str() {
            Some("none") => Parity::None,
            Some("even") => Parity::Even,
            Some("odd") => Parity::Odd,
            Some("mark") => Parity::Mark,
            Some("space") => Parity::Space,
            _ => {
                problems.push("parity");
                Parity::None
            }
        };
        let stop_bits = match value["stopBits"].as_f64() {
            Some(1.0) => StopBits::One,
            Some(1.5) => StopBits::OneHalf,
            Some(2.0) => StopBits::Two,
            _ => {
                problems.push("stopBits");
                StopBits::One
            }
        };
        if !problems.is_empty() {
            return Err(SerialOpenError::new(
                "invalidConfig",
                format!("Invalid serial config: {}", problems.join(", ")),
            ));
        }
        Ok(Self {
            baud_rate: baud_rate as u32,
            data_bits,
            parity,
            stop_bits,
            rtscts: value["rtscts"].as_bool().unwrap_or(false),
            auto_reconnect: value["autoReconnect"].as_bool().unwrap_or(false),
        })
    }

    /// 回给 Renderer 的 JSON（键序与 TS 的 `SerialConfig` 一致）。
    pub fn to_json(&self) -> Value {
        json!({
            "baudRate": self.baud_rate,
            "dataBits": self.data_bits,
            "parity": match self.parity {
                Parity::None => "none",
                Parity::Even => "even",
                Parity::Odd => "odd",
                Parity::Mark => "mark",
                Parity::Space => "space",
            },
            "stopBits": match self.stop_bits {
                StopBits::One => 1.0,
                StopBits::OneHalf => 1.5,
                StopBits::Two => 2.0,
            },
            "rtscts": self.rtscts,
            "autoReconnect": self.auto_reconnect,
        })
    }
}

/// TS `SerialErrorCode` 的打开相关子集。
#[derive(Clone, Debug, PartialEq)]
pub struct SerialOpenError {
    pub code: &'static str,
    pub message: String,
}

impl SerialOpenError {
    pub fn new(code: &'static str, message: String) -> Self {
        Self { code, message }
    }

    /// TS `mapSerialOpenError`：按消息映射稳定错误码。
    pub fn from_message(message: String) -> Self {
        let lower = message.to_lowercase();
        let matches = |patterns: &[&str]| patterns.iter().any(|pattern| lower.contains(pattern));
        let code = if matches(&[
            "access denied",
            "resource busy",
            "cannot lock port",
            "port is locked",
            "ebusy",
        ]) {
            "busy"
        } else if matches(&["permission denied", "eacces"]) {
            "denied"
        } else if matches(&["file not found", "no such file", "does not exist", "enoent"]) {
            "notFound"
        } else if matches(&["invalid argument", "einval"]) {
            "invalidConfig"
        } else {
            "io"
        };
        Self::new(code, message)
    }
}

/// 读写失败的形状（`SerialError` 的 `notOpen`/`io`/`invalidInput` 子集）。
#[derive(Clone, Debug, PartialEq)]
pub struct SerialIoError {
    pub code: &'static str,
    pub message: String,
}

impl SerialIoError {
    pub fn new(code: &'static str, message: String) -> Self {
        Self { code, message }
    }
}

/// 读循环句柄：关闭端口前先 abort，避免在已关闭的 fd 上继续 epoll。
pub struct ReaderHandle {
    task: tokio::task::JoinHandle<()>,
}

impl ReaderHandle {
    pub fn abort(&self) {
        self.task.abort();
    }
}

/// 已打开的端口句柄。
#[async_trait::async_trait]
pub trait SerialPort: Send + Sync {
    fn close(&self);
    /// 写入全部字节（非阻塞 fd 上 EAGAIN 时短重试，上限 5s）。
    async fn write(&self, bytes: Vec<u8>) -> Result<(), SerialIoError>;
    /// 启动读循环：读到字节即回调。句柄由端口自身持有，`close` 时先 abort 再关 fd。
    fn spawn_reader(&self, sink: std::sync::Arc<dyn Fn(Vec<u8>) + Send + Sync>);
}

/// 打开端口的传输抽象：生产实现是 termios，测试注入假实现。
#[async_trait::async_trait]
pub trait SerialTransport: Send + Sync {
    async fn open(
        &self,
        path: &str,
        config: &SerialConfig,
    ) -> Result<Box<dyn SerialPort>, SerialOpenError>;
}

#[cfg(target_os = "linux")]
#[path = "transport_native.rs"]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::NativeTransport;

/// 非 Linux：本片没有原生实现（登记的差异，见 spec §9）。
#[cfg(not(target_os = "linux"))]
pub struct NativeTransport;

#[cfg(not(target_os = "linux"))]
#[async_trait::async_trait]
impl SerialTransport for NativeTransport {
    async fn open(
        &self,
        _path: &str,
        _config: &SerialConfig,
    ) -> Result<Box<dyn SerialPort>, SerialOpenError> {
        Err(SerialOpenError::new(
            "nativeUnavailable",
            "Serial ports are only supported on Linux in this Host".to_owned(),
        ))
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "transport_testing.rs"]
pub(crate) mod testing;
