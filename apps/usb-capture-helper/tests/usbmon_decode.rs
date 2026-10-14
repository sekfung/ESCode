//! usbmon 记录解析与批处理计数的纯逻辑测试（不需要设备、不需要特权）。
//!
//! 构造 64 字节 `struct mon_bin_hdr`（小端，字段顺序按内核 mon_bin.c），验证方向、传输类型、
//! 长度、setup 字段；并覆盖三类必须跳过的记录：填充包 `'@'`、等时传输（计入 skippedIsochronous）、
//! 未知 tag / 未知传输类型。

use std::fs;
use std::path::PathBuf;

use escode_usb_capture::pcapng::{PcapngWriter, partial_path, write_records};
use escode_usb_capture::usbmon_bin::{
    Direction, EventKind, MON_HEADER_LEN, MonHeader, Record, TransferType, decode_record,
};

/// 构造一条 64 字节头 + 载荷的记录。
struct Builder {
    header: [u8; MON_HEADER_LEN],
    payload: Vec<u8>,
}

impl Builder {
    fn new(tag: u8, xfer_type: u8, epnum: u8) -> Self {
        let mut header = [0u8; MON_HEADER_LEN];
        header[8] = tag;
        header[9] = xfer_type;
        header[10] = epnum;
        header[11] = 4;
        header[12..14].copy_from_slice(&1u16.to_le_bytes());
        header[14] = b'-';
        header[15] = b'D';
        header[16..24].copy_from_slice(&1_700_000_000i64.to_le_bytes());
        header[24..28].copy_from_slice(&123_456i32.to_le_bytes());
        Self { header, payload: Vec::new() }
    }

    fn id(mut self, id: u64) -> Self {
        self.header[0..8].copy_from_slice(&id.to_le_bytes());
        self
    }

    fn bus(mut self, bus: u16) -> Self {
        self.header[12..14].copy_from_slice(&bus.to_le_bytes());
        self
    }

    fn device(mut self, device: u8) -> Self {
        self.header[11] = device;
        self
    }

    fn setup(mut self, setup: [u8; 8]) -> Self {
        self.header[14] = 0;
        self.header[40..48].copy_from_slice(&setup);
        self
    }

    fn status(mut self, status: i32) -> Self {
        self.header[28..32].copy_from_slice(&status.to_le_bytes());
        self
    }

    fn lengths(mut self, len_urb: u32, len_cap: u32) -> Self {
        self.header[32..36].copy_from_slice(&len_urb.to_le_bytes());
        self.header[36..40].copy_from_slice(&len_cap.to_le_bytes());
        self
    }

    fn flag_data(mut self, flag: u8) -> Self {
        self.header[15] = flag;
        self
    }

    fn payload(mut self, payload: &[u8]) -> Self {
        self.header[36..40].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.payload = payload.to_vec();
        self
    }

    /// 映射内的记录字节：头 + 载荷。截断场景由调用方自己切。
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MON_HEADER_LEN + self.payload.len());
        out.extend_from_slice(&self.header);
        out.extend_from_slice(&self.payload);
        out
    }

    fn header(&self) -> [u8; MON_HEADER_LEN] {
        self.header
    }
}

fn expect_event<'a>(record: &'a [u8]) -> escode_usb_capture::usbmon_bin::Event<'a> {
    match decode_record(record) {
        Record::Event(event) => event,
        other => panic!("期望事件，实际是 {}", describe(&other)),
    }
}

fn describe(record: &Record<'_>) -> &'static str {
    match record {
        Record::Event(_) => "事件",
        Record::Filler => "填充包",
        Record::Isochronous => "等时事件",
        Record::Unknown => "未知记录",
    }
}

#[test]
fn parses_control_submit_with_setup() {
    // GET_DESCRIPTOR(device)：bmRequestType=0x80、bRequest=6、wValue=0x0100、wIndex=0、wLength=8。
    let setup = [0x80, 6, 0x00, 0x01, 0x00, 0x00, 8, 0];
    let builder = Builder::new(b'S', 2, 0x80)
        .id(0xffff_8888_1234)
        .bus(3)
        .device(9)
        .setup(setup)
        .status(0)
        .lengths(8, 0);
    let bytes = builder.bytes();
    let event = expect_event(&bytes);

    assert_eq!(event.header.kind(), Some(EventKind::Submit));
    assert_eq!(event.header.transfer_type(), Some(TransferType::Control));
    assert_eq!(event.header.direction(), Direction::In);
    assert_eq!(event.header.busnum, 3);
    assert_eq!(event.header.devnum, 9);
    assert_eq!(event.header.id, 0xffff_8888_1234);
    assert_eq!(event.header.len_urb, 8);
    assert_eq!(event.header.len_cap, 0);
    assert_eq!(event.header.status, 0);
    assert_eq!(event.header.ts_micros, 1_700_000_000_123_456);
    let parsed = event.header.setup_packet().expect("控制提交带 setup");
    assert_eq!(parsed.bm_request_type, 0x80);
    assert_eq!(parsed.b_request, 6);
    assert_eq!(parsed.w_value, 0x0100);
    assert_eq!(parsed.w_index, 0);
    assert_eq!(parsed.w_length, 8);
    // 数据区前 64 字节必须是内核头原样（读侧从 header[40..48] 取 setup）。
    assert_eq!(&event.raw_header[40..48], &setup[..]);
    assert_eq!(event.payload.len(), 0);
    assert!(!event.payload_truncated);
}

#[test]
fn parses_control_complete_with_data_and_nonzero_status() {
    let payload = [
        0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x6d, 0x27, 0x01, 0x03, 0x00, 0x01, 0x02,
        0x03, 0x04, 0x05,
    ];
    let builder = Builder::new(b'C', 2, 0x80)
        .setup([0x80, 6, 0x00, 0x01, 0x00, 0x00, 18, 0])
        .status(-32)
        .payload(&payload);
    let bytes = builder.bytes();
    let event = expect_event(&bytes);

    assert_eq!(event.header.kind(), Some(EventKind::Complete));
    assert_eq!(event.header.transfer_type(), Some(TransferType::Control));
    assert_eq!(event.header.direction(), Direction::In);
    assert_eq!(event.header.len_urb, payload.len() as u32);
    assert_eq!(event.header.len_cap, payload.len() as u32);
    // 非零 status（-32 = -EPIPE，控制传输 STALL）必须原样保留，读侧靠它报异常。
    assert_eq!(event.header.status, -32);
    assert_eq!(event.payload, &payload[..]);
    // 内核只在控制传输的提交事件里填 setup；这条仍带 setup 字节，但状态是完成，语义由读侧决定。
    assert!(event.header.setup_packet().is_some());
}

#[test]
fn parses_bulk_out_submit_without_data_and_short_complete() {
    let submit = Builder::new(b'S', 3, 0x02).lengths(31, 0).flag_data(b'<').bytes();
    let event = expect_event(&submit);
    assert_eq!(event.header.transfer_type(), Some(TransferType::Bulk));
    assert_eq!(event.header.direction(), Direction::Out);
    assert_eq!(event.header.len_urb, 31);
    assert_eq!(event.header.len_cap, 0);
    assert!(event.payload.is_empty());
    assert!(event.header.setup_packet().is_none(), "批量传输没有 setup");

    // 短传输：声明 512、只抓到 64（读侧按 len_cap 取数据、按 len_urb 报 orig_len）。
    let complete = Builder::new(b'C', 3, 0x82).status(0).lengths(512, 64).payload(&[0x11; 64]).bytes();
    let event = expect_event(&complete);
    assert_eq!(event.header.direction(), Direction::In);
    assert_eq!(event.header.len_urb, 512);
    assert_eq!(event.header.len_cap, 64);
    assert_eq!(event.payload.len(), 64);
}

#[test]
fn parses_interrupt_and_error_records() {
    let interrupt = Builder::new(b'C', 1, 0x81).lengths(8, 8).payload(&[0x01; 8]).bytes();
    let event = expect_event(&interrupt);
    assert_eq!(event.header.transfer_type(), Some(TransferType::Interrupt));
    assert_eq!(event.header.direction(), Direction::In);

    // 错误事件：内核写 type='E'、flag_data='E'（没有数据）。
    let error = Builder::new(b'E', 3, 0x02).status(-2).lengths(0, 0).flag_data(b'E').bytes();
    let event = expect_event(&error);
    assert_eq!(event.header.kind(), Some(EventKind::Error));
    assert_eq!(event.header.status, -2);
    assert!(event.payload.is_empty());
}

#[test]
fn skips_filler_and_isochronous_and_unknown_records() {
    let filler = Builder::new(b'@', 3, 0).lengths(0, 4096 - MON_HEADER_LEN as u32).bytes();
    assert!(matches!(decode_record(&filler), Record::Filler));

    let iso_submit = Builder::new(b'S', 0, 0x81).lengths(1024, 1024).bytes();
    assert!(matches!(decode_record(&iso_submit), Record::Isochronous));
    let iso_complete = Builder::new(b'C', 0, 0x81).lengths(1024, 1024 + 4 * 16).bytes();
    assert!(matches!(decode_record(&iso_complete), Record::Isochronous));

    let bad_tag = Builder::new(b'X', 3, 0x02).lengths(0, 0).bytes();
    assert!(matches!(decode_record(&bad_tag), Record::Unknown));
    let bad_xfer = Builder::new(b'C', 7, 0x02).lengths(0, 0).bytes();
    assert!(matches!(decode_record(&bad_xfer), Record::Unknown));
    assert!(matches!(decode_record(&[0u8; MON_HEADER_LEN - 1]), Record::Unknown));
}

#[test]
fn marks_payload_truncated_at_ring_boundary() {
    let builder = Builder::new(b'C', 3, 0x82).lengths(64, 64).payload(&[0x22; 64]);
    let full = builder.bytes();
    // 模拟记录位于映射末尾：内核保证不跨缓冲，这里只验证防御行为不越界。
    let truncated = &full[..MON_HEADER_LEN + 10];
    let event = expect_event(truncated);
    assert!(event.payload_truncated);
    assert_eq!(event.payload.len(), 10);
    assert_eq!(event.payload, &[0x22; 10][..]);
}

#[test]
fn header_setup_requires_control_and_flag_setup_not_dash() {
    // 批量传输带 setup 字节：读侧同样不认（条件含 xfer_type == control）。
    let bulk = Builder::new(b'S', 3, 0x02).setup([0x80, 6, 0, 1, 0, 0, 8, 0]).lengths(8, 0).bytes();
    let header = MonHeader::parse(&bulk).expect("解析头");
    assert_eq!(header.flag_setup, 0);
    assert!(header.setup_packet().is_none());

    // 控制传输但没有 setup（flag_setup = '-'）时不当 setup 用。
    let control = Builder::new(b'S', 2, 0x80).lengths(8, 0).bytes();
    let header = MonHeader::parse(&control).expect("解析头");
    assert_eq!(header.flag_setup, b'-');
    assert!(header.setup_packet().is_none());
}

fn temp_dir(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("escode-usb-capture-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("创建测试临时目录");
    path
}

#[test]
fn write_records_counts_skipped_and_written_events() {
    let dir = temp_dir("write-records");
    let output = dir.join("mixed.pcapng");
    let mut writer = PcapngWriter::create(&output, "usbmon1").expect("创建写入器");
    let header_bytes = 28 + 32;

    let bulk_complete = Builder::new(b'C', 3, 0x82).lengths(64, 64).payload(&[0x33; 64]).bytes();
    let control_complete = Builder::new(b'C', 2, 0x80).status(-32).lengths(8, 8).payload(&[0x44; 8]).bytes();
    let filler = Builder::new(b'@', 3, 0).lengths(0, 1024).bytes();
    let iso = Builder::new(b'C', 0, 0x81).lengths(256, 256).bytes();
    let unknown = Builder::new(b'Q', 3, 0x82).lengths(0, 0).bytes();

    let records: Vec<&[u8]> =
        vec![&bulk_complete, &filler, &iso, &control_complete, &unknown, &iso];
    let stats = write_records(&mut writer, &records, 512 * 1024 * 1024).expect("写批次");
    assert_eq!(stats.packets, 2);
    assert_eq!(stats.skipped_isochronous, 2, "等时事件必须被跳过并计数");
    assert_eq!(stats.malformed, 1);
    assert!(!stats.reached_max_bytes);
    // 两条 EPB：64 字节头 + 载载荷；64 + 64 = 128 与 64 + 8 = 72 都已 4 字节对齐。
    assert_eq!(writer.bytes(), header_bytes + (32 + 128) + (32 + 72));
    let bytes = writer.finish(true).expect("收尾");
    assert_eq!(bytes, fs::metadata(&output).expect("stat").len());
    assert!(output.exists());
    assert!(!partial_path(&output).exists());
}

#[test]
fn write_records_stops_at_max_bytes_after_one_event() {
    let dir = temp_dir("write-records-max");
    let output = dir.join("max.pcapng");
    let mut writer = PcapngWriter::create(&output, "usbmon1").expect("创建写入器");
    let limit = writer.bytes() + 1;
    let first = Builder::new(b'C', 3, 0x82).lengths(64, 64).payload(&[0x55; 64]).bytes();
    let second = Builder::new(b'C', 3, 0x82).lengths(64, 64).payload(&[0x66; 64]).bytes();
    let stats = write_records(&mut writer, &[&first, &second], limit).expect("写批次");
    assert!(stats.reached_max_bytes);
    assert_eq!(stats.packets, 1, "达到上限后不再写后续事件");
    writer.finish(true).expect("收尾");
}
