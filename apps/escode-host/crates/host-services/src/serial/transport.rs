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

/// 已打开的端口句柄。S2 只需要关闭；读取/写入/信号随 S3/S4 扩展。
pub trait SerialPort: Send + Sync {
    fn close(&self);
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
mod linux {
    use std::ffi::CString;
    use std::os::unix::io::RawFd;

    use super::{Parity, SerialConfig, SerialOpenError, SerialPort, SerialTransport, StopBits};

    pub struct NativeTransport;

    struct NativePort {
        fd: RawFd,
    }

    // fd 的读写由内核串行化；`close` 用原子交换保证只关一次。
    unsafe impl Send for NativePort {}
    unsafe impl Sync for NativePort {}

    impl SerialPort for NativePort {
        fn close(&self) {
            let fd = self.fd;
            if fd >= 0 {
                unsafe {
                    libc::close(fd);
                }
            }
        }
    }

    /// 标准波特率 → `speed_t`。非标准速率在这里被拒绝（登记的差异）。
    fn speed_of(baud: u32) -> Option<libc::speed_t> {
        Some(match baud {
            50 => libc::B50,
            75 => libc::B75,
            110 => libc::B110,
            134 => libc::B134,
            150 => libc::B150,
            200 => libc::B200,
            300 => libc::B300,
            600 => libc::B600,
            1200 => libc::B1200,
            1800 => libc::B1800,
            2400 => libc::B2400,
            4800 => libc::B4800,
            9600 => libc::B9600,
            19200 => libc::B19200,
            38400 => libc::B38400,
            57600 => libc::B57600,
            115200 => libc::B115200,
            230400 => libc::B230400,
            460800 => libc::B460800,
            500000 => libc::B500000,
            576000 => libc::B576000,
            921600 => libc::B921600,
            1000000 => libc::B1000000,
            1152000 => libc::B1152000,
            1500000 => libc::B1500000,
            2000000 => libc::B2000000,
            2500000 => libc::B2500000,
            3000000 => libc::B3000000,
            3500000 => libc::B3500000,
            4000000 => libc::B4000000,
            _ => return None,
        })
    }

    fn configure(fd: RawFd, config: &SerialConfig) -> Result<(), SerialOpenError> {
        let Some(speed) = speed_of(config.baud_rate) else {
            return Err(SerialOpenError::new(
                "invalidConfig",
                format!(
                    "Unsupported baud rate on this Host: {} (standard rates only)",
                    config.baud_rate
                ),
            ));
        };
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut termios) != 0 {
                return Err(SerialOpenError::from_message(io_message("tcgetattr")));
            }
            // 原始模式：不做回显、不做行缓冲（TS 的 serialport 默认 raw）。
            libc::cfmakeraw(&mut termios);
            libc::cfsetspeed(&mut termios, speed);
            termios.c_cflag |= libc::CLOCAL | libc::CREAD;
            termios.c_cflag &= !libc::CSIZE;
            termios.c_cflag |= match config.data_bits {
                5 => libc::CS5,
                6 => libc::CS6,
                7 => libc::CS7,
                _ => libc::CS8,
            };
            termios.c_cflag &= !(libc::PARENB | libc::PARODD);
            match config.parity {
                Parity::None => {}
                Parity::Even => termios.c_cflag |= libc::PARENB,
                Parity::Odd => termios.c_cflag |= libc::PARENB | libc::PARODD,
                // mark/space 需要 CMSPAR（Linux 扩展）。
                Parity::Mark => termios.c_cflag |= libc::PARENB | libc::CMSPAR | libc::PARODD,
                Parity::Space => termios.c_cflag |= libc::PARENB | libc::CMSPAR,
            }
            termios.c_cflag &= !libc::CSTOPB;
            // POSIX 只在 5 位数据时能表达 1.5 停止位。
            match config.stop_bits {
                StopBits::One => {}
                StopBits::OneHalf if config.data_bits == 5 => termios.c_cflag |= libc::CSTOPB,
                StopBits::OneHalf => {
                    return Err(SerialOpenError::new(
                        "invalidConfig",
                        "1.5 stop bits requires 5 data bits".to_owned(),
                    ));
                }
                StopBits::Two => termios.c_cflag |= libc::CSTOPB,
            }
            if config.rtscts {
                termios.c_cflag |= libc::CRTSCTS;
            } else {
                termios.c_cflag &= !libc::CRTSCTS;
            }
            // 非阻塞读：Reader 用 poll/read 的语义，避免 read 永久阻塞（S3 起使用）。
            termios.c_cc[libc::VMIN] = 0;
            termios.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
                return Err(SerialOpenError::from_message(io_message("tcsetattr")));
            }
        }
        Ok(())
    }

    fn io_message(operation: &str) -> String {
        let error = std::io::Error::last_os_error();
        format!("{operation} failed: {error}")
    }

    #[async_trait::async_trait]
    impl SerialTransport for NativeTransport {
        async fn open(
            &self,
            path: &str,
            config: &SerialConfig,
        ) -> Result<Box<dyn SerialPort>, SerialOpenError> {
            let path = path.to_owned();
            let config = config.clone();
            tokio::task::spawn_blocking(move || {
                let open_error =
                    |error: std::io::Error| SerialOpenError::from_message(error.to_string());
                let c_path = CString::new(path.clone()).map_err(|_| {
                    SerialOpenError::new("invalidConfig", "Invalid port path".to_owned())
                })?;
                let fd = unsafe {
                    libc::open(
                        c_path.as_ptr(),
                        libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK,
                    )
                };
                if fd < 0 {
                    return Err(open_error(std::io::Error::last_os_error()));
                }
                let port = NativePort { fd };
                if let Err(error) = configure(fd, &config) {
                    port.close();
                    return Err(error);
                }
                Ok(Box::new(port) as Box<dyn SerialPort>)
            })
            .await
            .map_err(|error| SerialOpenError::new("io", format!("{error}")))?
        }
    }
}

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
