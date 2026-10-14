use escode_usb_capture::usbmon_bin::{MonHeader, MON_HEADER_LEN};
fn main() {
    let mut header = [0u8; MON_HEADER_LEN];
    header[8] = b'S';
    header[9] = 2;
    let parsed = MonHeader::parse(&header);
    println!("parsed={parsed:?}");
}
