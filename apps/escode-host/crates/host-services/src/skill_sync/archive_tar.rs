//! ustar 头与数字字段（TS `skillSyncArchive.ts` 的手写 tar）。
//!
//! 只覆盖 skill 同步需要的两种条目：目录（`5`，路径带尾 `/`）与普通文件（`0`）。
//! 字节布局与 TS 完全一致，保证两种 Host 互相读取归档。

use super::error::SkillSyncError;

pub(super) const BLOCK: usize = 512;

/// TS `writeTarOctal`：低 `length-1` 位八进制、左侧补零、末字节留 NUL。
pub(super) fn write_octal(buffer: &mut [u8], value: u64, offset: usize, length: usize) {
    let digits = format!("{value:o}");
    let keep = length - 1;
    let digits = if digits.len() > keep {
        &digits[digits.len() - keep..]
    } else {
        digits.as_str()
    };
    let padded = format!("{digits:0>keep$}");
    write_string(buffer, &padded, offset, keep);
}

/// TS `writeTarString`：按 UTF-8 写入，最多 `length` 字节，不拆字符。
pub(super) fn write_string(buffer: &mut [u8], value: &str, offset: usize, length: usize) {
    let mut written = 0;
    for ch in value.chars() {
        let mut scratch = [0_u8; 4];
        let bytes = ch.encode_utf8(&mut scratch).as_bytes();
        if written + bytes.len() > length {
            break;
        }
        buffer[offset + written..offset + written + bytes.len()].copy_from_slice(bytes);
        written += bytes.len();
    }
}

/// TS `createTarHeader`。
pub(super) fn header(
    entry_path: &str,
    mode: u64,
    mtime_seconds: u64,
    size: u64,
    type_flag: u8,
) -> Result<[u8; BLOCK], SkillSyncError> {
    let (name, prefix) = split_entry_path(entry_path)?;
    let mut buffer = [0_u8; BLOCK];
    write_string(&mut buffer, &name, 0, 100);
    write_octal(&mut buffer, mode & 0o777, 100, 8);
    write_octal(&mut buffer, 0, 108, 8);
    write_octal(&mut buffer, 0, 116, 8);
    write_octal(&mut buffer, size, 124, 12);
    write_octal(&mut buffer, mtime_seconds, 136, 12);
    buffer[148..156].fill(b' ');
    buffer[156] = type_flag;
    write_string(&mut buffer, "ustar", 257, 6);
    write_string(&mut buffer, "00", 263, 2);
    write_string(&mut buffer, "escode", 265, 32);
    write_string(&mut buffer, "escode", 297, 32);
    write_string(&mut buffer, &prefix, 345, 155);
    let checksum: u64 = buffer.iter().map(|byte| u64::from(*byte)).sum();
    let encoded = format!("{checksum:06o}");
    write_string(&mut buffer, &encoded, 148, 6);
    buffer[154] = 0;
    buffer[155] = 0x20;
    Ok(buffer)
}

/// TS `splitTarEntryPath`：>100 字节时从尾部找「prefix ≤155、name ≤100」的切分。
fn split_entry_path(entry_path: &str) -> Result<(String, String), SkillSyncError> {
    if entry_path.len() <= 100 {
        return Ok((entry_path.to_owned(), String::new()));
    }
    let segments: Vec<&str> = entry_path.split('/').collect();
    for index in (1..segments.len()).rev() {
        let prefix = segments[..index].join("/");
        let name = segments[index..].join("/");
        if prefix.len() <= 155 && name.len() <= 100 {
            return Ok((name, prefix));
        }
    }
    Err(SkillSyncError::plain(format!(
        "skill archive path is too long: {entry_path}"
    )))
}

/// TS `readTarEntryPath`。
pub(super) fn entry_path(header: &[u8; BLOCK]) -> String {
    let name = read_string(&header[0..100]);
    let prefix = read_string(&header[345..500]);
    if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    }
}

/// TS `readTarString`。
pub(super) fn read_string(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// TS `readTarOctal` + `Number.parseInt(_, 8)` 的前缀解析；非法或负数报错。
pub(super) fn parse_octal(field: &[u8]) -> Result<u64, SkillSyncError> {
    let text = read_string(field);
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    let (negative, digits) = match text.as_bytes()[0] {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let mut value: u64 = 0;
    let mut any = false;
    for char in digits.chars() {
        let Some(digit) = char.to_digit(8) else {
            break;
        };
        any = true;
        value = value
            .checked_mul(8)
            .and_then(|value| value.checked_add(u64::from(digit)))
            .ok_or_else(invalid_size)?;
    }
    if !any || negative || value > (1_u64 << 53) - 1 {
        return Err(invalid_size());
    }
    Ok(value)
}

fn invalid_size() -> SkillSyncError {
    SkillSyncError::plain("invalid skill sync archive entry size")
}

/// 512 字节对齐后的字节数。
pub(super) fn padded(size: u64) -> Result<u64, SkillSyncError> {
    size.checked_add(BLOCK as u64 - 1)
        .map(|value| value / BLOCK as u64 * BLOCK as u64)
        .ok_or_else(|| SkillSyncError::plain("invalid skill sync archive entry size"))
}

#[cfg(test)]
#[path = "archive_tar_tests.rs"]
mod tests;
