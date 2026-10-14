// 临时工具：不接设备生成一份 pcapng，用于和 TS 读侧交叉验证（验证后删除）。
use std::path::PathBuf;

use escode_usb_capture::pcapng::{PcapngWriter, UsbmonRecord, write_records};

fn header(
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
) -> [u8; 64] {
    let mut header = [0u8; 64];
    header[0..8].copy_from_slice(&0x0000_0000_dead_beefu64.to_le_bytes());
    header[8] = tag;
    header[9] = xfer_type;
    header[10] = epnum;
    header[11] = devnum;
    header[12..14].copy_from_slice(&1u16.to_le_bytes());
    header[14] = if xfer_type == 2 && tag == b'S' { 0 } else { b'-' };
    header[15] = b'D';
    header[16..24].copy_from_slice(&ts_sec.to_le_bytes());
    header[24..28].copy_from_slice(&ts_usec.to_le_bytes());
    header[28..32].copy_from_slice(&status.to_le_bytes());
    header[32..36].copy_from_slice(&len_urb.to_le_bytes());
    header[36..40].copy_from_slice(&len_cap.to_le_bytes());
    header[40..48].copy_from_slice(&setup);
    header
}

fn main() {
    let output = PathBuf::from(std::env::args().nth(1).expect("用法：write-sample <out.pcapng>"));
    let mut writer = PcapngWriter::create(&output, "usbmon1").expect("创建写入器");

    // 控制提交（GET_DESCRIPTOR device，wLength=18，带 setup）。
    let control_submit = header(
        b'S',
        2,
        0x80,
        3,
        1_700_000_000,
        100_000,
        0,
        18,
        0,
        [0x80, 6, 0x00, 0x01, 0x00, 0x00, 18, 0],
    );
    // 控制完成（返回 18 字节设备描述符，status = 0）。
    let descriptor: [u8; 18] = [
        0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x6d, 0x27, 0x01, 0x03, 0x00, 0x01, 0x02,
        0x03, 0x04, 0x05,
    ];
    let control_complete = header(b'C', 2, 0x80, 3, 1_700_000_000, 200_000, 0, 18, 18, [0; 8]);
    // 批量完成：短传输 + STALL 状态。
    let bulk_short = header(b'C', 3, 0x82, 3, 1_700_000_001, 500_000, -32, 512, 64, [0; 8]);
    // 等时完成：必须被跳过并计数。
    let isochronous = header(b'C', 0, 0x81, 3, 1_700_000_002, 0, 0, 1024, 1024 + 2 * 16, [0; 8]);
    // 填充包：跳过。
    let filler = header(b'@', 3, 0, 0, 0, 0, 0, 0, 4096 - 64, [0; 8]);

    let control_submit_bytes = {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&control_submit);
        bytes
    };
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
