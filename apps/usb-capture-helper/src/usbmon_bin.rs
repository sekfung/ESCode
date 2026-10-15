//! usbmon 二进制 API 的读取循环（Linux）。
//!
//! ABI 对照内核 `drivers/usb/mon/mon_bin.c`（本次实现时逐条核对过源码）：
//! - 设备节点 `/dev/usbmonN`，N 是总线号；**内核在 open / ioctl / mmap 路径上没有任何 capability
//!   检查**，访问完全由设备节点权限决定 → 本工具不需要也不使用 `setcap`。
//! - `MON_IOCQ_RING_SIZE`（`_IO(0x92, 5)`）拿环形缓冲大小，`mmap(PROT_READ, MAP_SHARED)` 映射；
//!   只读映射，写映射会被内核拒绝。
//! - `MON_IOCX_MFETCH`（`_IOWR(0x92, 7, struct mon_bin_mfetch)`）批量取事件：`offvec` 是**用户指针**
//!   数组，返回映射内偏移；返回后内核把实际条数写回 `nfetch`。缓冲为空时该 ioctl 会睡眠，
//!   所以先 `poll(POLLIN)` 再取（`mon_bin_poll` 在环形缓冲非空时报可读）。**它不消费环形缓冲**
//!   （内核注释：the events are usually freed later with `mon_bin_flush`），取完必须用
//!   `MON_IOCH_MFLUSH`（`_IO(0x92, 8)`，参数是事件条数）释放，否则下一轮会重复拿到同一批事件。
//! - 偏移处是 64 字节 `struct mon_bin_hdr`，载荷紧跟其后（+64）。记录不跨缓冲末尾：内核用
//!   `type == '@'` 的填充包占位（`mon_buff_area_fill`），填充包跳过即可。
//! - `MON_IOCG_STATS`（`_IOR(0x92, 3, struct mon_bin_stats)`）读丢弃计数；内核读一次就清零 `cnt_lost`，
//!   所以收尾时读一次即「本次抓包期间因环形缓冲满而丢的事件数」。
//!
//! ⚠️ **未在真机验证**：open / ioctl / mmap / poll 这条路径需要 root 或已授权的 usbmon 设备节点，
//! 开发环境既没有加载 usbmon 也没有 `/dev/usbmon*`。因此这里把纯逻辑（64 字节头解析、事件归一化、
//! 批处理计数）与 unsafe 薄层分开：纯逻辑在 tests/ 里全面覆盖，真机复核步骤见 README「未验证部分」。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::error::{HelperError, syscall_error, usbmon_open_error};
use crate::pcapng::{PcapngWriter, partial_path, validate_output_path, write_records};

/// `struct mon_bin_hdr` 的长度（内核 `PKT_SIZE`，同时是记录的 4 字节对齐粒度）。
pub const MON_HEADER_LEN: usize = 64;
/// 一次 `MON_IOCX_MFETCH` 最多取的事件数：批量摊薄 ioctl 开销，又不至于让单次拷贝太大。
const BATCH_EVENTS: usize = 512;
/// 每次 poll 的最长等待：决定 duration / 信号收尾的最大延迟。
const POLL_SLICE_MS: i32 = 100;

pub const MIN_BUS: u32 = 1;
/// USB 总线号在 usbmon 设备名里是十进制整数，实际枚举范围远小于 255；上界按保守值校验。
pub const MAX_BUS: u32 = 255;
pub const MIN_DURATION_MS: u64 = 1;
pub const MAX_DURATION_MS: u64 = 60_000;
pub const MIN_MAX_BYTES: u64 = 1;
pub const MAX_MAX_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// 提交阶段（内核 `type == 'S'`）。
    Submit,
    /// 完成阶段（`'C'`）。
    Complete,
    /// 错误（`'E'`，如提交被 odm 拒绝/取消）。
    Error,
}

impl EventKind {
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            b'S' => Some(Self::Submit),
            b'C' => Some(Self::Complete),
            b'E' => Some(Self::Error),
            _ => None,
        }
    }
}

/// 传输类型：内核 `xfer_type` 存的是 PIPE 常量（ISO=0、Intr=1、Control=2、Bulk=3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    Isochronous,
    Interrupt,
    Control,
    Bulk,
}

impl TransferType {
    pub fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Isochronous),
            1 => Some(Self::Interrupt),
            2 => Some(Self::Control),
            3 => Some(Self::Bulk),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setup {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}

/// 64 字节 `struct mon_bin_hdr` 的解析结果（小端：读侧 `usbmon.ts` 也按小端解）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonHeader {
    pub id: u64,
    pub tag: u8,
    pub xfer_type: u8,
    pub epnum: u8,
    pub devnum: u8,
    pub busnum: u16,
    pub flag_setup: u8,
    pub flag_data: u8,
    pub ts_micros: u64,
    pub status: i32,
    pub len_urb: u32,
    pub len_cap: u32,
    pub setup: [u8; 8],
}

impl MonHeader {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < MON_HEADER_LEN {
            return None;
        }
        let mut setup = [0u8; 8];
        setup.copy_from_slice(&bytes[40..48]);
        Some(Self {
            id: u64::from_le_bytes(bytes[0..8].try_into().ok()?),
            tag: bytes[8],
            xfer_type: bytes[9],
            epnum: bytes[10],
            devnum: bytes[11],
            busnum: u16::from_le_bytes(bytes[12..14].try_into().ok()?),
            flag_setup: bytes[14],
            flag_data: bytes[15],
            // 头部自带 wall-clock 时间戳（ktime_get_real_ts64）；ts_sec 是 s64、ts_usec 是 s32。
            ts_micros: (i64::from_le_bytes(bytes[16..24].try_into().ok()?) as u64)
                .wrapping_mul(1_000_000)
                .wrapping_add(i32::from_le_bytes(bytes[24..28].try_into().ok()?) as u64),
            status: i32::from_le_bytes(bytes[28..32].try_into().ok()?),
            len_urb: u32::from_le_bytes(bytes[32..36].try_into().ok()?),
            len_cap: u32::from_le_bytes(bytes[36..40].try_into().ok()?),
            setup,
        })
    }

    pub fn kind(&self) -> Option<EventKind> {
        EventKind::from_tag(self.tag)
    }

    pub fn transfer_type(&self) -> Option<TransferType> {
        TransferType::from_raw(self.xfer_type)
    }

    /// bit7 = 方向（1 = IN）。
    pub fn direction(&self) -> Direction {
        if self.epnum & 0x80 != 0 {
            Direction::In
        } else {
            Direction::Out
        }
    }

    /// 填充包：内核在记录可能跨缓冲末尾时用 `'@'` 占位。
    pub fn is_filler(&self) -> bool {
        self.tag == b'@'
    }

    /// 与读侧同一条件：控制传输且 `flag_setup != '-'`（内核只在控制传输的提交事件里填 setup，
    /// 其它情况写 `'-'`）。提交阶段之外的数据即使非零也不当 setup 用。
    pub fn setup_packet(&self) -> Option<Setup> {
        if self.transfer_type() != Some(TransferType::Control) || self.flag_setup == b'-' {
            return None;
        }
        Some(Setup {
            bm_request_type: self.setup[0],
            b_request: self.setup[1],
            w_value: u16::from_le_bytes([self.setup[2], self.setup[3]]),
            w_index: u16::from_le_bytes([self.setup[4], self.setup[5]]),
            w_length: u16::from_le_bytes([self.setup[6], self.setup[7]]),
        })
    }
}

#[derive(Debug)]
pub struct Event<'a> {
    pub header: MonHeader,
    /// 内核给的 64 字节头原样字节（`header[40..48]` 是 setup，读侧从这里取）。
    pub raw_header: [u8; MON_HEADER_LEN],
    pub payload: &'a [u8],
    /// `len_cap` 大于可用字节数：正常不会发生（记录不跨缓冲末尾），出现即视为防御性命中。
    pub payload_truncated: bool,
}

#[derive(Debug)]
pub enum Record<'a> {
    Event(Event<'a>),
    /// 填充包，跳过。
    Filler,
    /// 等时传输，整体跳过并计数（`len_cap` 含尾部 ISO 描述符，长度语义复杂且本工具不解码等时协议）。
    Isochronous,
    /// 头不足 64 字节、tag 或 xfer_type 未知：正常内核不会产生，出现即跳过并计数。
    Unknown,
}

/// 解析一条记录。`bytes` 是「该记录在映射内的偏移到映射末尾」的切片：切片长度就是可用字节数，
/// 因此尾部截断（理论上不会发生）会被 `payload_truncated` 如实标出而不是越界读。
pub fn decode_record(bytes: &[u8]) -> Record<'_> {
    let Some(header) = MonHeader::parse(bytes) else {
        return Record::Unknown;
    };
    if header.is_filler() {
        return Record::Filler;
    }
    let Some(transfer) = header.transfer_type() else {
        return Record::Unknown;
    };
    if header.kind().is_none() {
        return Record::Unknown;
    }
    if transfer == TransferType::Isochronous {
        return Record::Isochronous;
    }
    let mut raw_header = [0u8; MON_HEADER_LEN];
    raw_header.copy_from_slice(&bytes[..MON_HEADER_LEN]);
    let available = bytes.len() - MON_HEADER_LEN;
    let declared = header.len_cap as usize;
    let payload_len = declared.min(available);
    Record::Event(Event {
        header,
        raw_header,
        payload: &bytes[MON_HEADER_LEN..MON_HEADER_LEN + payload_len],
        payload_truncated: declared > available,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Duration,
    MaxBytes,
    Signal,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duration => "duration",
            Self::MaxBytes => "maxBytes",
            Self::Signal => "signal",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub bus: u32,
    pub duration_ms: u64,
    pub max_bytes: u64,
    pub output: PathBuf,
}

#[derive(Debug)]
pub struct CaptureOutcome {
    pub packets: u64,
    pub bytes: u64,
    /// 内核 `MON_IOCG_STATS.dropped`（本次抓包期间因缓冲满丢弃的事件数）。
    pub dropped: u32,
    pub skipped_isochronous: u64,
    pub stopped_by: StopReason,
    /// 实际落盘路径：正常收尾是 `--output`；被信号中断时是 `<output>.partial`。
    pub output: PathBuf,
    pub bus: u32,
}

static STOP_SIGNAL: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_stop_signal(_signal: libc::c_int) {
    // 信号处理器里只做 async-signal-safe 的一件事：置标志，主循环下一次醒来时收尾。
    STOP_SIGNAL.store(true, Ordering::SeqCst);
}

/// 安装 SIGINT/SIGTERM 处理器：不设 SA_RESTART，让阻塞中的 poll/ioctl 以 EINTR 返回，
/// 最坏也只多等一个 poll 周期（`POLL_SLICE_MS`）就能收尾。
#[cfg(unix)]
pub fn install_signal_handlers() -> Result<(), HelperError> {
    // SAFETY: 处理器只写一个 AtomicBool；sa_mask 先清零再交给内核，句柄与 flags 都是本进程常量。
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handle_stop_signal as *const () as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = 0;
        for signal in [libc::SIGINT, libc::SIGTERM] {
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(HelperError::io(format!(
                    "安装信号处理器失败：{}",
                    std::io::Error::last_os_error()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn install_signal_handlers() -> Result<(), HelperError> {
    Ok(())
}

pub fn stop_requested() -> bool {
    STOP_SIGNAL.load(Ordering::SeqCst)
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::CString;
    use std::io;
    use std::mem::size_of;
    use std::os::fd::RawFd;

    use super::*;

    const MON_IOC_MAGIC: u32 = 0x92;
    const IOC_NONE: u32 = 0;
    const IOC_WRITE: u32 = 1;
    const IOC_READ: u32 = 2;

    /// include/uapi/asm-generic/ioctl.h 的 `_IOC()`：方向 2 位 | 长度 14 位 | 类型 8 位 | 序号 8 位。
    /// 在编译期算出数值，避免把 32/64 位不同的魔数抄错。
    const fn ioc(dir: u32, nr: u32, size: u32) -> libc::c_ulong {
        ((dir << 30) | (size << 16) | (MON_IOC_MAGIC << 8) | nr) as libc::c_ulong
    }

    /// `struct mon_bin_mfetch { u32 __user *offvec; u32 nfetch; u32 nflush; }`
    /// —— `offvec` 是用户指针，所以结构体大小随指针宽度变化，ioctl 号也随之变化。
    #[repr(C)]
    struct MonBinMfetch {
        offvec: *mut u32,
        nfetch: u32,
        nflush: u32,
    }

    /// `struct mon_bin_stats { u32 queued; u32 dropped; }`
    #[repr(C)]
    #[derive(Debug, Default, Clone, Copy)]
    pub struct MonBinStats {
        pub queued: u32,
        pub dropped: u32,
    }

    // 与内核 mon_bin.c 的 `_IOC` 结果一致；x86_64 上的实际取值：0x9205 / 0xC0109207 / 0x80089203 / 0x9208。
    const MON_IOCQ_RING_SIZE: libc::c_ulong = ioc(IOC_NONE, 5, 0);
    const MON_IOCX_MFETCH: libc::c_ulong =
        ioc(IOC_READ | IOC_WRITE, 7, size_of::<MonBinMfetch>() as u32);
    const MON_IOCG_STATS: libc::c_ulong = ioc(IOC_READ, 3, size_of::<MonBinStats>() as u32);
    const MON_IOCH_MFLUSH: libc::c_ulong = ioc(IOC_NONE, 8, 0);

    pub struct UsbmonDevice {
        fd: RawFd,
        map: *const u8,
        ring_size: usize,
        offsets: Vec<u32>,
    }

    impl UsbmonDevice {
        pub fn open(bus: u32) -> Result<Self, HelperError> {
            let path = format!("/dev/usbmon{bus}");
            let c_path = CString::new(path.as_str()).expect("设备路径只含 ASCII 数字");
            // SAFETY: 只读打开 usbmon 字符设备；路径由受校验的总线号拼出，不会为空指针。
            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
            if fd < 0 {
                return Err(usbmon_open_error(bus, &io::Error::last_os_error()));
            }
            // SAFETY: MON_IOCQ_RING_SIZE 是无参 _IO，返回环形缓冲大小；失败返回负值。
            let ring = unsafe { libc::ioctl(fd, MON_IOCQ_RING_SIZE) };
            if ring <= 0 {
                // SAFETY: fd 由上面的 open 返回且尚未交给 Self 管理，此处必须关闭，否则泄漏。
                unsafe { libc::close(fd) };
                return Err(HelperError::io(format!(
                    "MON_IOCQ_RING_SIZE 返回非法大小 {ring}（期望正数）"
                )));
            }
            let ring_size = ring as usize;
            // SAFETY: 只读共享映射环形缓冲；内核 mmap 实现拒绝 PROT_WRITE。
            let mapping = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    ring_size,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            if mapping == libc::MAP_FAILED {
                let error = io::Error::last_os_error();
                // SAFETY: 同上，mmap 失败后必须关闭 fd。
                unsafe { libc::close(fd) };
                return Err(syscall_error("mmap /dev/usbmon 环形缓冲失败", &error));
            }
            Ok(Self {
                fd,
                map: mapping.cast::<u8>(),
                ring_size,
                offsets: vec![0; BATCH_EVENTS],
            })
        }

        /// `poll(POLLIN)`：环形缓冲里有事件（或填充包）时返回 true，超时返回 false。
        /// 信号打断（EINTR）也返回 false，由主循环检查停止标志。
        pub fn wait_readable(&self, timeout_ms: i32) -> Result<bool, HelperError> {
            let mut poll_fd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: 指向栈上单个 pollfd，nfds = 1。
            let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    return Ok(false);
                }
                return Err(syscall_error("poll(/dev/usbmon) 失败", &error));
            }
            Ok(ready > 0 && poll_fd.revents & libc::POLLIN != 0)
        }

        /// 一次 `MON_IOCX_MFETCH`：内核把映射内偏移写进 `offsets`，并把实际条数写回 `nfetch`。
        /// 返回本批条数；`offsets()` 在同一借用期内给出这些偏移。
        ///
        /// 注意：这个 ioctl **不消费**环形缓冲（内核注释：the events are usually freed later with
        /// `mon_bin_flush`），取完必须 `release()` 释放，否则下一轮会拿到同一批事件。
        pub fn fetch(&mut self) -> Result<usize, HelperError> {
            let mut mfetch = MonBinMfetch {
                offvec: self.offsets.as_mut_ptr(),
                nfetch: self.offsets.len() as u32,
                nflush: 0,
            };
            // SAFETY: offvec 指向本结构体持有的、长度足够的 Vec；nfetch 是它的容量。
            let result = unsafe { libc::ioctl(self.fd, MON_IOCX_MFETCH, &mut mfetch) };
            if result < 0 {
                return Err(syscall_error(
                    "MON_IOCX_MFETCH 失败",
                    &io::Error::last_os_error(),
                ));
            }
            Ok((mfetch.nfetch as usize).min(self.offsets.len()))
        }

        /// `MON_IOCH_MFLUSH`（`_IO(0x92, 8)`）：释放环形缓冲头部的 `events` 条事件。
        /// 参数与返回值都是**事件条数**（内核 `mon_bin_flush(rp, nevents)`），按 `b_out` 顺序释放，
        /// 所以「释放刚取走的那一批」正好把它们从环里清掉。
        pub fn release(&self, events: u32) -> Result<u32, HelperError> {
            // SAFETY: MON_IOCH_MFLUSH 接收一个无符号长整型的条数，无指针对参数。
            let released =
                unsafe { libc::ioctl(self.fd, MON_IOCH_MFLUSH, events as libc::c_ulong) };
            if released < 0 {
                return Err(syscall_error(
                    "MON_IOCH_MFLUSH 失败",
                    &io::Error::last_os_error(),
                ));
            }
            Ok(released as u32)
        }

        pub fn offsets(&self) -> &[u32] {
            &self.offsets
        }

        /// 映射内 `[offset, ring_size)` 的字节；越界偏移（不该出现）返回 None 而不是读越界。
        pub fn record_bytes(&self, offset: u32) -> Option<&[u8]> {
            let start = offset as usize;
            if start.checked_add(MON_HEADER_LEN)? > self.ring_size {
                return None;
            }
            // SAFETY: mmap 的区域在 Self 存活期间有效且至少 ring_size 字节；start < ring_size 已校验。
            unsafe {
                Some(std::slice::from_raw_parts(
                    self.map.add(start),
                    self.ring_size - start,
                ))
            }
        }

        /// 丢弃计数。内核在这个 ioctl 里读走并清零 `cnt_lost`，所以只能在收尾时调用一次。
        pub fn stats(&self) -> Result<MonBinStats, HelperError> {
            let mut stats = MonBinStats::default();
            // SAFETY: 输出缓冲是栈上的 MonBinStats，大小与内核 copy_to_user 的长度一致。
            let result = unsafe { libc::ioctl(self.fd, MON_IOCG_STATS, &mut stats) };
            if result < 0 {
                return Err(syscall_error(
                    "MON_IOCG_STATS 失败",
                    &io::Error::last_os_error(),
                ));
            }
            Ok(stats)
        }
    }

    impl Drop for UsbmonDevice {
        fn drop(&mut self) {
            // SAFETY: fd 与映射都由 Self 持有、只在这里释放一次；munmap 长度是映射时的大小。
            unsafe {
                libc::munmap(self.map as *mut libc::c_void, self.ring_size);
                let _ = libc::close(self.fd);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{MonBinStats, UsbmonDevice};

/// 受限参数校验放在这里而不是只放在 CLI：库接口也要挡住越界值（docs/specs/usb-agent-tools.md §4.1
/// 「只接受固定子命令与受限参数（bus 范围、duration/bytes 上限）」）。
#[cfg(target_os = "linux")]
fn validate_request(request: &CaptureRequest) -> Result<(), HelperError> {
    if !(MIN_BUS..=MAX_BUS).contains(&request.bus) {
        return Err(HelperError::invalid_input(format!(
            "总线号 {0} 越界（允许 {MIN_BUS}–{MAX_BUS}）",
            request.bus
        )));
    }
    if !(MIN_DURATION_MS..=MAX_DURATION_MS).contains(&request.duration_ms) {
        return Err(HelperError::invalid_input(format!(
            "抓包时长 {0}ms 越界（允许 {MIN_DURATION_MS}–{MAX_DURATION_MS}）",
            request.duration_ms
        )));
    }
    if !(MIN_MAX_BYTES..=MAX_MAX_BYTES).contains(&request.max_bytes) {
        return Err(HelperError::invalid_input(format!(
            "体积上限 {0} 越界（允许 {MIN_MAX_BYTES}–{MAX_MAX_BYTES}）",
            request.max_bytes
        )));
    }
    validate_output_path(&request.output)
}

#[cfg(target_os = "linux")]
pub fn run_capture(request: &CaptureRequest) -> Result<CaptureOutcome, HelperError> {
    validate_request(request)?;
    // 先开设备：打不开时（不存在/无权限）不要留下空的 .partial 文件。
    let mut device = UsbmonDevice::open(request.bus)?;
    let interface = format!("usbmon{}", request.bus);
    let mut writer = PcapngWriter::create(&request.output, &interface)?;
    let deadline = Instant::now() + Duration::from_millis(request.duration_ms);
    let mut packets = 0u64;
    let mut skipped_isochronous = 0u64;
    let mut malformed = 0u64;
    let stopped_by;
    'capture: loop {
        if stop_requested() {
            stopped_by = StopReason::Signal;
            break 'capture;
        }
        if Instant::now() >= deadline {
            stopped_by = StopReason::Duration;
            break 'capture;
        }
        if writer.bytes() >= request.max_bytes {
            stopped_by = StopReason::MaxBytes;
            break 'capture;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait_ms = (remaining.as_millis().min(POLL_SLICE_MS as u128)).max(1) as i32;
        if !device.wait_readable(wait_ms)? {
            continue;
        }
        let count = device.fetch()?;
        if count == 0 {
            continue;
        }
        // 先把本批偏移落成切片（映射生命周期内有效），归入统计的语义在 pcapng::write_records 里。
        let offsets = device.offsets();
        let mut batch: Vec<&[u8]> = Vec::with_capacity(count);
        for offset in &offsets[..count] {
            match device.record_bytes(*offset) {
                Some(bytes) => batch.push(bytes),
                None => malformed += 1,
            }
        }
        let stats = write_records(&mut writer, &batch, request.max_bytes)?;
        packets += stats.packets;
        skipped_isochronous += stats.skipped_isochronous;
        malformed += stats.malformed;
        // 取走的事件要显式释放（MFETCH 不消费环形缓冲）：跳过的填充包/等时事件同样占用环空间。
        let released = device.release(count as u32)?;
        if released != count as u32 {
            eprintln!(
                "警告：释放了 {released} 条事件，但本批取走了 {count} 条（环形缓冲状态可能已不同步）"
            );
        }
        if stats.reached_max_bytes {
            stopped_by = StopReason::MaxBytes;
            break 'capture;
        }
    }
    let dropped = match device.stats() {
        Ok(stats) => stats.dropped,
        Err(error) => {
            // 收尾失败不改变抓包结果，但必须让调用方看到「丢弃计数缺失」而不是静默当成 0。
            eprintln!("警告：读取 MON_IOCG_STATS 失败，本轮 dropped 上报为 0：{error}");
            0
        }
    };
    if malformed > 0 {
        eprintln!("警告：跳过 {malformed} 条无法识别的 usbmon 记录（正常内核不会产生）");
    }
    // 被信号中断时保留 .partial：半截文件由所有者（插件）决定如何处理，本工具不假装它是完整产物。
    let interrupted = stopped_by == StopReason::Signal;
    let bytes = writer.finish(!interrupted)?;
    Ok(CaptureOutcome {
        packets,
        bytes,
        dropped,
        skipped_isochronous,
        stopped_by,
        output: if interrupted {
            partial_path(&request.output)
        } else {
            request.output.clone()
        },
        bus: request.bus,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn run_capture(_request: &CaptureRequest) -> Result<CaptureOutcome, HelperError> {
    Err(HelperError::new(
        ErrorCode::NoInterface,
        "本抓包器只支持 Linux usbmon（Windows 走独立的抓包服务）。",
    )
    .with_remediation("在 Linux 上使用内置抓包器，或改用 dumpcap 兼容后端 / 离线解码路径。"))
}
