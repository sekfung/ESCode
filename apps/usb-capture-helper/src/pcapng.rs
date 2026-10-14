//! pcapng 写入器（docs/specs/usb-agent-tools.md §4.1）。
//!
//! 契约以读侧为准：`apps/escode-cli/packages/usb-plugin/src/pcap/pcapng.ts`（容器）与
//! `.../pcap/usbmon.ts`（`LINKTYPE_USB_LINUX_MMAPPED = 220` 链路层，字段语义按内核 `struct mon_bin_hdr`）。
//! 纯函数（返回字节块）+ 一层文件写入：块布局可以被单测逐字段校验，不依赖设备。
//!
//! 两个不显然的点：
//! - EPB 时间戳是 64 位整数拆成「高 32 位字段在前、低 32 位字段在后」的两段，各自按段字节序存放，
//!   而不是把一个 u64 直接按小端写出（后者会变成低字在前，读侧按高字在前还原，30 万秒以上就会对不上）。
//! - 链路类型 220 的数据区必须原样包含内核给的 64 字节头，载荷紧跟其后：读侧从 `header[40..48]`
//!   取 setup、从 `header[36]` 取 len_cap，再按它截取载荷。

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::error::HelperError;

/// `LINKTYPE_USB_LINUX_MMAPPED`（usbmon 二进制 API 的头 + 数据）。
pub const LINKTYPE_USB_LINUX_MMAPPED: u16 = 220;
/// `struct mon_bin_hdr` 的长度（内核 `PKT_SIZE`）。
pub const USB_MON_HEADER_LEN: usize = 64;
/// 与 dumpcap 抓 usbmon 时用的 snaplen 一致；usbmon 单条记录不可能超过它。
pub const SNAPLEN: u32 = 262_144;
/// 每个数据包的 pcapng 固定开销：块头 8 + interface_id/时间戳/cap/orig 20 + 尾部长度 4。
const EPB_OVERHEAD: u64 = 32;

const BLOCK_SECTION_HEADER: u32 = 0x0A0D_0D0A;
const BLOCK_INTERFACE_DESCRIPTION: u32 = 1;
const BLOCK_ENHANCED_PACKET: u32 = 6;
const BYTE_ORDER_MAGIC: u32 = 0x1A2B_3C4D;
const OPT_IF_NAME: u16 = 2;

/// 一条待写入的 usbmon 记录：数据区 = 内核 64 字节头原样 + 载荷。
pub struct UsbmonRecord<'a> {
    /// 事件时间戳（µs）：头部 `ts_sec * 1_000_000 + ts_usec`。
    pub ts_micros: u64,
    pub header: &'a [u8; USB_MON_HEADER_LEN],
    pub payload: &'a [u8],
    /// 头部 `len_urb`（URB 的声明长度，用于 `orig_len`；与 `cap_len` 不等即短传输）。
    pub declared_length: u32,
}

/// SHB：段头块，28 字节，声明小端、1.0 版本、长度不限（-1）。
pub fn section_header_block() -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&BYTE_ORDER_MAGIC.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&u64::MAX.to_le_bytes());
    block(BLOCK_SECTION_HEADER, &body)
}

/// IDB：接口描述块，声明 220 并附 `if_name` 选项。
///
/// 选项区写到最后一个选项为止，不追加 `opt_endofopt`：读侧在「剩余不足 4 字节」时结束扫描，
/// 这样 `if_name = "usbmonN"` 时块长度正好是 32 字节（与契约一致）。
pub fn interface_description_block(if_name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(24);
    body.extend_from_slice(&LINKTYPE_USB_LINUX_MMAPPED.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&SNAPLEN.to_le_bytes());
    let name = if_name.as_bytes();
    body.extend_from_slice(&OPT_IF_NAME.to_le_bytes());
    // 名字固定是 "usbmonN"（≤ 9 字节），u16 长度字段不会溢出。
    body.extend_from_slice(&(name.len() as u16).to_le_bytes());
    body.extend_from_slice(name);
    pad_to_4(&mut body);
    block(BLOCK_INTERFACE_DESCRIPTION, &body)
}

/// EPB：增强包块。`data` 已经包含「64 字节头 + 载荷」。
pub fn enhanced_packet_block(ts_micros: u64, data: &[u8], original_length: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(20 + data.len() + 4);
    body.extend_from_slice(&0u32.to_le_bytes());
    // 高字在前（见模块注释：不是 u64 小端）。
    body.extend_from_slice(&((ts_micros >> 32) as u32).to_le_bytes());
    body.extend_from_slice(&(ts_micros as u32).to_le_bytes());
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(&original_length.to_le_bytes());
    body.extend_from_slice(data);
    pad_to_4(&mut body);
    block(BLOCK_ENHANCED_PACKET, &body)
}

fn block(block_type: u32, body: &[u8]) -> Vec<u8> {
    debug_assert_eq!(body.len() % 4, 0, "块体必须按 4 字节对齐");
    let total_length = (body.len() + 12) as u32;
    let mut out = Vec::with_capacity(total_length as usize);
    out.extend_from_slice(&block_type.to_le_bytes());
    out.extend_from_slice(&total_length.to_le_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(&total_length.to_le_bytes());
    out
}

fn pad_to_4(buffer: &mut Vec<u8>) {
    while !buffer.len().is_multiple_of(4) {
        buffer.push(0);
    }
}

pub fn partial_path(output: &Path) -> PathBuf {
    let mut name = output.as_os_str().to_os_string();
    name.push(".partial");
    PathBuf::from(name)
}

/// `output` 的校验错误一律是 `invalidInput`：路径由调用方给出，属于输入问题。
///
/// 安全约束（docs/specs/usb-agent-tools.md §4.1「拒绝符号链接与非 .pcapng 后缀」）：
/// 后缀必须是 `.pcapng`；输出文件本身与其直接父目录都不能是符号链接，避免提权场景下
/// 被参数化的路径写到别处；父目录必须存在且是目录（可写性由创建 `.partial` 时真实检验）。
pub fn validate_output_path(output: &Path) -> Result<(), HelperError> {
    let name = output
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| HelperError::invalid_input("输出路径缺少文件名"))?;
    if !name.to_ascii_lowercase().ends_with(".pcapng") {
        return Err(HelperError::invalid_input(format!(
            "输出路径必须以 .pcapng 结尾：{name}"
        )));
    }
    let parent = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let parent_meta = fs::symlink_metadata(parent).map_err(|error| {
        HelperError::invalid_input(format!("输出目录不可用：{}（{error}）", parent.display()))
    })?;
    if parent_meta.file_type().is_symlink() {
        return Err(HelperError::invalid_input(format!(
            "拒绝符号链接形式的输出目录：{}",
            parent.display()
        )));
    }
    if !parent_meta.is_dir() {
        return Err(HelperError::invalid_input(format!(
            "输出路径的父级不是目录：{}",
            parent.display()
        )));
    }
    match fs::symlink_metadata(output) {
        Ok(meta) if meta.file_type().is_symlink() => Err(HelperError::invalid_input(format!(
            "拒绝符号链接形式的输出文件：{}",
            output.display()
        ))),
        Ok(meta) if meta.is_dir() => Err(HelperError::invalid_input(format!(
            "输出路径是目录：{}",
            output.display()
        ))),
        // 已存在的普通文件允许覆盖：先写 .partial，再改名。
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(HelperError::invalid_input(format!(
            "无法检查输出路径：{}（{error}）",
            output.display()
        ))),
    }
}

/// 先写 `<output>.partial`，正常收尾时才改名为 `<output>`：半截文件被当成完整抓包分析比失败更糟。
pub struct PcapngWriter {
    file: BufWriter<File>,
    partial: PathBuf,
    final_path: PathBuf,
    bytes: u64,
}

impl PcapngWriter {
    pub fn create(output: &Path, interface_name: &str) -> Result<Self, HelperError> {
        validate_output_path(output)?;
        let partial = partial_path(output);
        let file = File::create(&partial).map_err(|error| {
            HelperError::invalid_input(format!(
                "无法创建输出文件：{}（{error}）",
                partial.display()
            ))
        })?;
        let mut writer = Self {
            file: BufWriter::new(file),
            partial,
            final_path: output.to_path_buf(),
            bytes: 0,
        };
        writer.write_block(&section_header_block())?;
        writer.write_block(&interface_description_block(interface_name))?;
        Ok(writer)
    }

    fn write_block(&mut self, block: &[u8]) -> Result<(), HelperError> {
        self.file
            .write_all(block)
            .map_err(|error| HelperError::io(format!("写入抓包文件失败：{error}")))?;
        self.bytes += block.len() as u64;
        Ok(())
    }

    /// 已写入的字节数（含 SHB/IDB）；用于 `maxBytes` 上限判断与结果上报。
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// 写一条事件。`cap_len = 64 + payload.len()`，`orig_len = 64 + declared_length`。
    pub fn write_event(&mut self, record: &UsbmonRecord<'_>) -> Result<(), HelperError> {
        let mut data = Vec::with_capacity(USB_MON_HEADER_LEN + record.payload.len());
        data.extend_from_slice(record.header);
        data.extend_from_slice(record.payload);
        let original_length = USB_MON_HEADER_LEN as u32 + record.declared_length;
        let block = enhanced_packet_block(record.ts_micros, &data, original_length);
        self.write_block(&block)
    }

    /// 单个事件写入后会占用的最大字节数（按 4 字节对齐后的上限），用于提前判断体积上限。
    pub fn max_event_bytes(payload_len: usize) -> u64 {
        EPB_OVERHEAD + (USB_MON_HEADER_LEN + payload_len).next_multiple_of(4) as u64
    }

    /// 收尾：flush + fsync。`rename` 为 true 时把 `.partial` 改成最终文件；被中断时传 false，保留 `.partial`。
    pub fn finish(mut self, rename: bool) -> Result<u64, HelperError> {
        self.file
            .flush()
            .map_err(|error| HelperError::io(format!("刷新抓包文件失败：{error}")))?;
        self.file
            .get_ref()
            .sync_all()
            .map_err(|error| HelperError::io(format!("fsync 抓包文件失败：{error}")))?;
        if rename {
            fs::rename(&self.partial, &self.final_path).map_err(|error| {
                HelperError::io(format!(
                    "把 {} 改名为 {} 失败：{error}",
                    self.partial.display(),
                    self.final_path.display()
                ))
            })?;
        }
        Ok(self.bytes)
    }
}

/// 批处理统计：一次 `MON_IOCX_MFETCH` 取回的一组记录的实际归入情况。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BatchStats {
    pub packets: u64,
    pub skipped_isochronous: u64,
    pub malformed: u64,
    /// 本批写入过程中达到 `max_bytes`，调用方应停止抓包。
    pub reached_max_bytes: bool,
}

/// 把一批映射内的记录写进 pcapng。纯逻辑（不碰 fd / 映射），因此计数语义可以被单测覆盖：
/// 填充包（'@'，内核用来防止记录跨环形缓冲末尾）静默跳过，等时传输整体跳过并计数
/// （它的 `len_cap` 含尾部 ISO 描述符，长度语义复杂，宁可如实跳过也不写长度可疑的数据）。
pub fn write_records(
    writer: &mut PcapngWriter,
    records: &[&[u8]],
    max_bytes: u64,
) -> Result<BatchStats, HelperError> {
    let mut stats = BatchStats::default();
    for record in records {
        match crate::usbmon_bin::decode_record(record) {
            crate::usbmon_bin::Record::Filler => {}
            crate::usbmon_bin::Record::Isochronous => stats.skipped_isochronous += 1,
            crate::usbmon_bin::Record::Unknown => stats.malformed += 1,
            crate::usbmon_bin::Record::Event(event) => {
                writer.write_event(&UsbmonRecord {
                    ts_micros: event.header.ts_micros,
                    header: &event.raw_header,
                    payload: event.payload,
                    declared_length: event.header.len_urb,
                })?;
                stats.packets += 1;
                if writer.bytes() >= max_bytes {
                    stats.reached_max_bytes = true;
                    return Ok(stats);
                }
            }
        }
    }
    Ok(stats)
}
