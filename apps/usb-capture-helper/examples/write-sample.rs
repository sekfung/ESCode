//! 不接设备生成一份样例 pcapng，用于在没有 USB 设备的机器上复核写入器。
//!
//! ```text
//! cargo run --example write-sample -- /tmp/sample.pcapng
//! ```
//!
//! 生成的内容覆盖几条最容易写错的路径：控制传输（SETUP 在头部里、数据在完成侧）、批量短传输与
//! STALL 状态（`status = -32`）、等时事件（必须被跳过并计数）、填充包（必须被跳过）。
//! 校验方式二选一：用 Wireshark 打开看字段，或交给 ESCode 的 `usb_decode` 与仓库 TS 读侧解析。
//!
//! 为什么值得留着：真实抓包只能在有 root + usbmon 的机器上拿到（见 README「未验证部分」），
//! 而「写入器自身有没有写对」必须在任何机器上都能自证。

use std::path::PathBuf;

use escode_usb_capture::pcapng::{PcapngWriter, UsbmonRecord, write_records};

/// `struct mon_bin_hdr` 里的字段，取值语义与内核一致（小端）。
#[derive(Default)]
struct HeaderSpec {
    tag: u8,
    xfer_type: u8,
    epnum: u8,
    devnum: u8,
    ts_sec: i64,
    ts_usec: i32,
    status: i32,
    len_urb: u32,
    len_cap: u32,
    setup: [u8; 8],
}

fn header(spec: HeaderSpec) -> [u8; 64] {
    let mut header = [0u8; 64];
    header[0..8].copy_from_slice(&0x0000_0000_dead_beefu64.to_le_bytes());
    header[8] = spec.tag;
    header[9] = spec.xfer_type;
    header[10] = spec.epnum;
    header[11] = spec.devnum;
    header[12..14].copy_from_slice(&1u16.to_le_bytes());
    header[14] = if spec.xfer_type == 2 && spec.tag == b'S' {
        0
    } else {
        b'-'
    };
    header[15] = b'D';
    header[16..24].copy_from_slice(&spec.ts_sec.to_le_bytes());
    header[24..28].copy_from_slice(&spec.ts_usec.to_le_bytes());
    header[28..32].copy_from_slice(&spec.status.to_le_bytes());
    header[32..36].copy_from_slice(&spec.len_urb.to_le_bytes());
    header[36..40].copy_from_slice(&spec.len_cap.to_le_bytes());
    header[40..48].copy_from_slice(&spec.setup);
    header
}

fn main() {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("用法：write-sample <out.pcapng>"),
    );
    let mut writer = PcapngWriter::create(&output, "usbmon1").expect("创建写入器");

    // 控制提交（GET_DESCRIPTOR device，wLength=18，带 setup）。
    let control_submit = header(HeaderSpec {
        tag: b'S',
        xfer_type: 2,
        epnum: 0x80,
        devnum: 3,
        ts_sec: 1_700_000_000,
        ts_usec: 100_000,
        len_urb: 18,
        setup: [0x80, 6, 0x00, 0x01, 0x00, 0x00, 18, 0],
        ..HeaderSpec::default()
    });
    // 控制完成（返回 18 字节设备描述符，status = 0）。
    let descriptor: [u8; 18] = [
        0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x6d, 0x27, 0x01, 0x03, 0x00, 0x01, 0x02,
        0x03, 0x04, 0x05,
    ];
    let control_complete = header(HeaderSpec {
        tag: b'C',
        xfer_type: 2,
        epnum: 0x80,
        devnum: 3,
        ts_sec: 1_700_000_000,
        ts_usec: 200_000,
        len_urb: 18,
        len_cap: 18,
        ..HeaderSpec::default()
    });
    // 批量完成：短传输（声明 512、实际 64）+ STALL 状态。
    let bulk_short = header(HeaderSpec {
        tag: b'C',
        xfer_type: 3,
        epnum: 0x82,
        devnum: 3,
        ts_sec: 1_700_000_001,
        ts_usec: 500_000,
        status: -32,
        len_urb: 512,
        len_cap: 64,
        ..HeaderSpec::default()
    });
    // 等时完成：必须被跳过并计数（len_cap 含尾部 ISO 描述符）。
    let isochronous = header(HeaderSpec {
        tag: b'C',
        epnum: 0x81,
        devnum: 3,
        ts_sec: 1_700_000_002,
        len_urb: 1024,
        len_cap: 1024 + 2 * 16,
        ..HeaderSpec::default()
    });
    // 填充包：内核在记录跨缓冲末尾时用它占位，读侧必须跳过。
    let filler = header(HeaderSpec {
        tag: b'@',
        xfer_type: 3,
        len_cap: 4096 - 64,
        ..HeaderSpec::default()
    });

    let mut control_submit_bytes = Vec::new();
    control_submit_bytes.extend_from_slice(&control_submit);
    let mut control_complete_bytes = Vec::new();
    control_complete_bytes.extend_from_slice(&control_complete);
    control_complete_bytes.extend_from_slice(&descriptor);
    let mut bulk_bytes = Vec::new();
    bulk_bytes.extend_from_slice(&bulk_short);
    bulk_bytes.extend_from_slice(&[0x5a; 64]);
    let isochronous_bytes = isochronous.to_vec();
    let filler_bytes = filler.to_vec();

    let records: Vec<&[u8]> = vec![
        &control_submit_bytes,
        &control_complete_bytes,
        &filler_bytes,
        &isochronous_bytes,
        &bulk_bytes,
    ];
    let stats = write_records(&mut writer, &records, 512 * 1024 * 1024).expect("写批次");
    let bytes = writer.finish(true).expect("收尾");
    println!(
        "{} packets={} skippedIso={} malformed={} bytes={}",
        output.display(),
        stats.packets,
        stats.skipped_isochronous,
        stats.malformed,
        bytes
    );
    let _ = UsbmonRecord {
        ts_micros: 0,
        header: &control_submit,
        payload: &[],
        declared_length: 0,
    };
}
