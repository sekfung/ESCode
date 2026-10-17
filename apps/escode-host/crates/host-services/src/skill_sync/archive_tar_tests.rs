//! ustar 头（`rust-u2-sync.md` §1.4）。
use super::*;

#[test]
fn header_round_trips_name_mode_size_and_checksum() {
    let block = header("group/skill/SKILL.md", 0o644, 1_700_000_000, 42, b'0').unwrap();
    assert_eq!(entry_path(&block), "group/skill/SKILL.md");
    assert_eq!(parse_octal(&block[100..108]).unwrap(), 0o644);
    assert_eq!(parse_octal(&block[124..136]).unwrap(), 42);
    assert_eq!(parse_octal(&block[136..148]).unwrap(), 1_700_000_000);
    assert_eq!(&block[257..263], b"ustar\0");
    assert_eq!(&block[263..265], b"00");
    // 校验和：把 chksum 字段当空格重算。
    let mut copy = block;
    copy[148..156].fill(b' ');
    let sum: u64 = copy.iter().map(|byte| u64::from(*byte)).sum();
    assert_eq!(&block[148..154], format!("{sum:06o}").as_bytes());
    assert_eq!(block[154], 0);
    assert_eq!(block[155], b' ');
}

#[test]
fn long_paths_split_into_prefix_and_name() {
    let long = format!("{}/{}", "a".repeat(80), "b".repeat(80));
    let block = header(&long, 0o755, 0, 0, b'5').unwrap();
    assert_eq!(entry_path(&block), long);
    let error = header(&"x".repeat(101), 0o755, 0, 0, b'5').unwrap_err();
    assert!(error.message.starts_with("skill archive path is too long:"));
}

#[test]
fn octal_parsing_matches_number_parse_int() {
    assert_eq!(parse_octal(b"0000644 \0").unwrap(), 0o644);
    assert_eq!(parse_octal(b"  0").unwrap(), 0);
    assert_eq!(parse_octal(b"12x").unwrap(), 0o12, "parseInt 前缀解析");
    for bad in [b"abc".as_slice(), b"-5", b"99999999999999999999"] {
        assert_eq!(
            parse_octal(bad).unwrap_err().message,
            "invalid skill sync archive entry size"
        );
    }
}
