//! pcapng 写入的独立校验。
//!
//! 读取器在本文件里**单独实现**（不 import 生产代码里的任何解析函数），逐字段对照契约：
//! 与 `apps/escode-cli/packages/usb-plugin/src/pcap/pcapng.ts` 的读法一致——块类型/两侧长度、
//! linktype=220、snaplen、`if_name` 选项编码、EPB 的 interface_id、时间戳高字在前、cap/orig 长度、
//! 数据区（64 字节头 + 载荷）、4 字节对齐。

use std::fs;
use std::path::PathBuf;

use escode_usb_capture::pcapng::{
    LINKTYPE_USB_LINUX_MMAPPED, PcapngWriter, SNAPLEN, USB_MON_HEADER_LEN, UsbmonRecord,
    enhanced_packet_block, interface_description_block, partial_path, section_header_block,
};

const BLOCK_SECTION_HEADER: u32 = 0x0A0D_0D0A;
const BLOCK_INTERFACE_DESCRIPTION: u32 = 1;
const BLOCK_ENHANCED_PACKET: u32 = 6;
const BYTE_ORDER_MAGIC: u32 = 0x1A2B_3C4D;

/// 测试内独立的 pcapng 块读取器。
struct Block<'a> {
    block_type: u32,
    total_length: u32,
    body: &'a [u8],
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn u64_at(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().expect("8 字节"))
}

/// 逐块扫描：长度必须 4 字节对齐，且块首尾长度字段一致（否则读侧会把文件当截断）。
fn parse_blocks(data: &[u8]) -> Vec<Block<'_>> {
    let mut blocks = Vec::new();
    let mut offset = 0;
    while offset + 12 <= data.len() {
        let block_type = u32_at(data, offset);
        let total_length = u32_at(data, offset + 4);
        assert_eq!(total_length % 4, 0, "偏移 {offset} 的块长度未按 4 字节对齐：{total_length}");
        assert!(total_length >= 12, "偏移 {offset} 的块长度非法：{total_length}");
        assert!(
            offset + total_length as usize <= data.len(),
            "偏移 {offset} 的块越过文件末尾"
        );
        assert_eq!(
            u32_at(data, offset + total_length as usize - 4),
            total_length,
            "偏移 {offset} 的块首尾长度不一致"
        );
        blocks.push(Block {
            block_type,
            total_length,
            body: &data[offset + 8..offset + total_length as usize - 4],
        });
        offset += total_length as usize;
    }
    assert_eq!(offset, data.len(), "文件尾部有未识别的字节");
    blocks
}

/// 构造一条 64 字节 usbmon 头（小端，按 `struct mon_bin_hdr` 的字段顺序）。
#[allow(clippy::too_many_arguments)]
fn mon_header(
    tag: u8,
    xfer_type: u8,
    epnum: u8,
    devnum: u8,
    busnum: u16,
    ts_sec: i64,
    ts_usec: i32,
    status: i32,
    len_urb: u32,
    len_cap: u32,
    setup: [u8; 8],
) -> [u8; USB_MON_HEADER_LEN] {
    let mut header = [0u8; USB_MON_HEADER_LEN];
    header[0..8].copy_from_slice(&0x1234_5678_9abc_def0u64.to_le_bytes());
    header[8] = tag;
    header[9] = xfer_type;
    header[10] = epnum;
    header[11] = devnum;
    header[12..14].copy_from_slice(&busnum.to_le_bytes());
    header[14] = 0;
    header[15] = b'D';
    header[16..24].copy_from_slice(&ts_sec.to_le_bytes());
    header[24..28].copy_from_slice(&ts_usec.to_le_bytes());
    header[28..32].copy_from_slice(&status.to_le_bytes());
    header[32..36].copy_from_slice(&len_urb.to_le_bytes());
    header[36..40].copy_from_slice(&len_cap.to_le_bytes());
    header[40..48].copy_from_slice(&setup);
    header
}

fn temp_dir(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("escode-usb-capture-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("创建测试临时目录");
    path
}

#[test]
fn section_header_block_matches_contract() {
    let block = section_header_block();
    assert_eq!(block.len(), 28, "SHB 必须是 28 字节");
    let blocks = parse_blocks(&block);
    let shb = &blocks[0];
    assert_eq!(shb.block_type, BLOCK_SECTION_HEADER);
    assert_eq!(shb.total_length, 28);
    assert_eq!(u32_at(shb.body, 0), BYTE_ORDER_MAGIC);
    assert_eq!(u16_at(shb.body, 4), 1, "major 版本");
    assert_eq!(u16_at(shb.body, 6), 0, "minor 版本");
    assert_eq!(u64_at(shb.body, 8), u64::MAX, "section length 不限");
}

#[test]
fn interface_description_block_encodes_linktype_and_if_name() {
    let block = interface_description_block("usbmon1");
    // 8 字节定长字段 + (选项头 4 + 名字 7 + 填充 1) + 尾部长度 4 = 32。
    assert_eq!(block.len(), 32, "if_name = usbmon1 时 IDB 应为 32 字节");
    let blocks = parse_blocks(&block);
    let idb = &blocks[0];
    assert_eq!(idb.block_type, BLOCK_INTERFACE_DESCRIPTION);
    assert_eq!(u16_at(idb.body, 0), LINKTYPE_USB_LINUX_MMAPPED);
    assert_eq!(LINKTYPE_USB_LINUX_MMAPPED, 220);
    assert_eq!(u16_at(idb.body, 2), 0, "reserved");
    assert_eq!(u32_at(idb.body, 4), SNAPLEN);
    assert_eq!(SNAPLEN, 262_144);
    // if_name 选项：code=2、长度是名字字节数、值按 4 字节对齐（含 0 填充）。
    assert_eq!(u16_at(idb.body, 8), 2, "选项 code");
    assert_eq!(u16_at(idb.body, 10), 7, "选项长度不含填充");
    assert_eq!(&idb.body[12..19], b"usbmon1");
    assert_eq!(idb.body[19], 0, "名字的 4 字节对齐填充");

    // 更长的名字（usbmon100，9 字节）要填充到 12 字节，块长度随之变成 36，仍然 4 字节对齐。
    let long = interface_description_block("usbmon100");
    assert_eq!(long.len(), 36);
    let parsed = parse_blocks(&long);
    let body = parsed[0].body;
    assert_eq!(u16_at(body, 10), 9);
    assert_eq!(&body[12..21], b"usbmon100");
    assert!(body[21..24].iter().all(|byte| *byte == 0), "名字填充必须是 0");
}

#[test]
fn enhanced_packet_block_layout_and_timestamp_high_word_first() {
    let mut header = mon_header(b'C', 3, 0x81, 4, 1, 1_700_000_000, 123_456, 0, 8, 8, [0; 8]);
    header[14] = b'-';
    let payload = [0xaa, 0xbb, 0xcc];
    let mut data = Vec::new();
    data.extend_from_slice(&header);
    data.extend_from_slice(&payload);
    // ts = 1_700_000_000 * 1e6 + 123_456，远大于 2^32：高低字顺序错了就会露馅。
    let ts_micros = 1_700_000_000_123_456u64;
    let block = enhanced_packet_block(ts_micros, &data, 72);

    let blocks = parse_blocks(&block);
    let epb = &blocks[0];
    assert_eq!(epb.block_type, BLOCK_ENHANCED_PACKET);
    assert_eq!(u32_at(epb.body, 0), 0, "interface_id");
    // 契约：timestamp_high 在偏移 12，timestamp_low 在偏移 16（不是 u64 小端）。
    assert_eq!(u32_at(epb.body, 4), (ts_micros >> 32) as u32);
    assert_eq!(u32_at(epb.body, 8), ts_micros as u32);
    assert_eq!(u32_at(epb.body, 12), data.len() as u32, "cap_len = 64 + 载荷");
    assert_eq!(u32_at(epb.body, 16), 72, "orig_len = 64 + len_urb");
    assert_eq!(&epb.body[20..20 + USB_MON_HEADER_LEN], &header[..], "数据区前 64 字节是内核头原样");
    assert_eq!(&epb.body[20 + USB_MON_HEADER_LEN..20 + data.len()], &payload[..]);
    assert_eq!(epb.total_length % 4, 0);
    assert_eq!(epb.total_length, 32 + (data.len() as u32).next_multiple_of(4));
    // 67 字节数据区要补 1 个 0 才是 4 的倍数。
    assert_eq!(epb.body[20 + data.len()], 0, "载荷后的对齐填充");
}

#[test]
fn writer_writes_two_events_and_renames_partial() {
    let dir = temp_dir("writer-two-events");
    let output = dir.join("capture.pcapng");
    let partial = partial_path(&output);
    let mut writer = PcapngWriter::create(&output, "usbmon7").expect("创建写入器");
    assert!(partial.exists(), "写入期间只有 .partial");
    assert!(!output.exists());

    let header_a = mon_header(b'S', 2, 0x80, 4, 7, 1_700_000_010, 1, 0, 8, 8, [0x80, 6, 0, 1, 0, 0, 8, 0]);
    let payload_a = [0u8; 8];
    writer
        .write_event(&UsbmonRecord {
            ts_micros: 1_700_000_010_000_001,
            header: &header_a,
            payload: &payload_a,
            declared_length: 8,
        })
        .expect("写第一条");
    let header_b = mon_header(b'C', 3, 0x02, 4, 7, 1_700_000_011, 500_000, -32, 512, 64, [0; 8]);
    let payload_b = vec![0x5a; 64];
    writer
        .write_event(&UsbmonRecord {
            ts_micros: 1_700_000_011_500_000,
            header: &header_b,
            payload: &payload_b,
            declared_length: 512,
        })
        .expect("写第二条");
    let reported_bytes = writer.bytes();
    let written = writer.finish(true).expect("收尾");
    assert_eq!(written, reported_bytes);
    assert!(output.exists(), "正常收尾后必须是最终文件名");
    assert!(!partial.exists(), "改名后不应残留 .partial");
    assert_eq!(fs::metadata(&output).expect("stat").len(), reported_bytes);

    let data = fs::read(&output).expect("读抓包文件");
    let blocks = parse_blocks(&data);
    assert_eq!(blocks.len(), 4, "SHB + IDB + 两个 EPB");

    let shb = &blocks[0];
    assert_eq!(shb.block_type, BLOCK_SECTION_HEADER);
    assert_eq!(shb.total_length, 28);
    assert_eq!(u32_at(shb.body, 0), BYTE_ORDER_MAGIC);

    let idb = &blocks[1];
    assert_eq!(idb.block_type, BLOCK_INTERFACE_DESCRIPTION);
    assert_eq!(u16_at(idb.body, 0), 220);
    assert_eq!(u32_at(idb.body, 4), SNAPLEN);
    assert_eq!(u16_at(idb.body, 8), 2);
    assert_eq!(u16_at(idb.body, 10), 7);
    assert_eq!(&idb.body[12..19], b"usbmon7");

    let first = &blocks[2];
    assert_eq!(first.block_type, BLOCK_ENHANCED_PACKET);
    assert_eq!(u32_at(first.body, 0), 0);
    assert_eq!(u32_at(first.body, 4), (1_700_000_010_000_001u64 >> 32) as u32);
    assert_eq!(u32_at(first.body, 8), 1_700_000_010_000_001u64 as u32);
    assert_eq!(u32_at(first.body, 12), 72);
    assert_eq!(u32_at(first.body, 16), 72);
    assert_eq!(&first.body[20..28], &header_a[..8]);
    assert_eq!(&first.body[20..84], &header_a[..]);
    assert_eq!(&first.body[84..92], &payload_a[..]);

    let second = &blocks[3];
    assert_eq!(second.block_type, BLOCK_ENHANCED_PACKET);
    assert_eq!(u32_at(second.body, 12), 128, "cap_len = 64 + 64");
    assert_eq!(u32_at(second.body, 16), 576, "orig_len = 64 + 512");
    assert_eq!(&second.body[20..84], &header_b[..]);
    assert_eq!(&second.body[84..148], &payload_b[..]);
    assert_eq!(second.total_length, 160, "32 + 128");
}
