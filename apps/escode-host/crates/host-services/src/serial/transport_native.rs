//! Linux 原生串口传输（termios + AsyncFd + modem 控制线）。由 `transport.rs` 以 `mod linux` 引入。

use std::ffi::CString;
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicI32, Ordering};

use super::super::signals::Signals;
use super::{
    Parity, SerialConfig, SerialIoError, SerialOpenError, SerialPort, SerialTransport, StopBits,
};

pub struct NativeTransport;

struct NativePort {
    /// `-1` 表示已关闭：`close` 用原子交换保证只关一次（重复关闭是空操作）。
    fd: AtomicI32,
    reader: std::sync::Mutex<Option<super::ReaderHandle>>,
}

// fd 的读写由内核串行化；`close` 用原子交换保证只关一次。
unsafe impl Send for NativePort {}
unsafe impl Sync for NativePort {}

impl NativePort {
    fn open_fd(&self) -> Result<RawFd, SerialIoError> {
        let fd = self.fd.load(Ordering::SeqCst);
        if fd < 0 {
            return Err(not_open());
        }
        Ok(fd)
    }
}

impl Drop for NativePort {
    fn drop(&mut self) {
        self.close();
    }
}

#[async_trait::async_trait]
impl SerialPort for NativePort {
    fn close(&self) {
        // 先停读循环，再关 fd（否则 epoll 会在已关闭的 fd 上继续触发）。
        if let Ok(mut slot) = self.reader.lock()
            && let Some(handle) = slot.take()
        {
            handle.abort();
        }
        let fd = self.fd.swap(-1, Ordering::SeqCst);
        if fd >= 0 {
            unsafe {
                libc::close(fd);
            }
        }
    }

    async fn write(&self, bytes: Vec<u8>) -> Result<(), SerialIoError> {
        let fd = self.open_fd()?;
        tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut written = 0_usize;
            while written < bytes.len() {
                let count = unsafe {
                    libc::write(
                        fd,
                        bytes[written..].as_ptr() as *const libc::c_void,
                        bytes.len() - written,
                    )
                };
                if count > 0 {
                    written += count as usize;
                    continue;
                }
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                }
                return Err(super::SerialIoError::new("io", error.to_string()));
            }
            Ok(())
        })
        .await
        .map_err(|error| super::SerialIoError::new("io", format!("{error}")))?
    }

    async fn set_signals(&self, signals: Signals) -> Result<(), SerialIoError> {
        let fd = self.open_fd()?;
        tokio::task::spawn_blocking(move || {
            let mut bits: libc::c_int = 0;
            if unsafe { libc::ioctl(fd, libc::TIOCMGET, &mut bits) } < 0 {
                return Err(io_error("TIOCMGET"));
            }
            let set = |bits: libc::c_int, flag: libc::c_int, on: bool| {
                if on { bits | flag } else { bits & !flag }
            };
            bits = set(bits, libc::TIOCM_DTR, signals.dtr);
            bits = set(bits, libc::TIOCM_RTS, signals.rts);
            if unsafe { libc::ioctl(fd, libc::TIOCMSET, &bits) } < 0 {
                return Err(io_error("TIOCMSET"));
            }
            Ok(())
        })
        .await
        .map_err(|error| super::SerialIoError::new("io", format!("{error}")))?
    }

    fn spawn_reader(&self, sink: std::sync::Arc<dyn Fn(Vec<u8>) + Send + Sync>) {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let Ok(fd) = self.open_fd() else {
            return;
        };
        // dup 一份给读循环：关闭原 fd 时读循环能通过 abort 先退出。
        let duplicated = unsafe { libc::dup(fd) };
        if duplicated < 0 {
            return;
        }
        let owned = unsafe { OwnedFd::from_raw_fd(duplicated) };
        let task = tokio::spawn(async move {
            // SAFETY: owned 拥有一个有效的 dup fd，在 AsyncFd 持有期间不会被关闭。
            let Ok(async_fd) = (unsafe { tokio::io::unix::AsyncFd::register(owned) }) else {
                return;
            };
            let mut buffer = vec![0_u8; 8192];
            loop {
                let mut guard = match async_fd.readable().await {
                    Ok(guard) => guard,
                    Err(_) => break,
                };
                let count = unsafe {
                    libc::read(
                        async_fd.as_raw_fd(),
                        buffer.as_mut_ptr() as *mut libc::c_void,
                        buffer.len(),
                    )
                };
                if count > 0 {
                    sink(buffer[..count as usize].to_vec());
                    continue;
                }
                if count == 0 {
                    guard.clear_ready();
                    continue;
                }
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                }
                break;
            }
        });
        let handle = super::ReaderHandle { task };
        if let Ok(mut slot) = self.reader.lock() {
            *slot = Some(handle);
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

fn io_error(operation: &str) -> SerialIoError {
    SerialIoError::new("io", io_message(operation))
}

fn not_open() -> SerialIoError {
    SerialIoError::new("notOpen", "Serial port is not open".to_owned())
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
            let port = NativePort {
                fd: AtomicI32::new(fd),
                reader: std::sync::Mutex::new(None),
            };
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
