//! ustar 头与数字字段（TS `pluginSyncArchive.ts` 的手写 tar）。

pub(super) const BLOCK: usize = 512;

pub(super) fn header(
    entry_path: &str,
    mode: u64,
    mtime_seconds: u64,
    size: u64,
    type_flag: u8,
) -> Result<[u8; BLOCK], String> {
    let (name, prefix) = split_entry_path(entry_path)?;
    let mut buffer = [0_u8; BLOCK];
    write_string(&mut buffer, &name, 0, 100)?;
    write_octal(&mut buffer, mode & 0o777, 100, 8);
    write_octal(&mut buffer, 0, 108, 8);
    write_octal(&mut buffer, 0, 116, 8);
    write_octal(&mut buffer, size, 124, 12);
    write_octal(&mut buffer, mtime_seconds, 136, 12);
    buffer[148..156].fill(b' ');
    buffer[156] = type_flag;
    write_string(&mut buffer, "ustar", 257, 6)?;
    write_string(&mut buffer, "00", 263, 2)?;
    write_string(&mut buffer, "escode", 265, 32)?;
    write_string(&mut buffer, "escode", 297, 32)?;
    write_string(&mut buffer, &prefix, 345, 155)?;
    let checksum: u64 = buffer.iter().map(|byte| u64::from(*byte)).sum();
    write_string(&mut buffer, &format!("{checksum:06o}"), 148, 6)?;
    buffer[154] = 0;
    buffer[155] = 0x20;
    Ok(buffer)
}

pub(super) fn split_entry_path(entry_path: &str) -> Result<(String, String), String> {
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
    Err(format!("plugin archive path is too long: {entry_path}"))
}

pub(super) fn entry_path(header: &[u8; BLOCK]) -> String {
    let name = read_string(&header[0..100]);
    let prefix = read_string(&header[345..500]);
    if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    }
}

pub(super) fn read_string(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

pub(super) fn parse_octal(field: &[u8]) -> Result<u64, String> {
    let text = read_string(field);
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    let text = text.trim_start_matches(['+', '-']);
    let mut value = 0u64;
    for character in text.chars() {
        let Some(digit) = character.to_digit(8) else {
            break;
        };
        value = value
            .checked_mul(8)
            .and_then(|value| value.checked_add(u64::from(digit)))
            .ok_or_else(|| "invalid plugin sync archive entry size".to_owned())?;
    }
    Ok(value)
}

pub(super) fn read_mode(header: &[u8; BLOCK]) -> u64 {
    parse_octal(&header[100..108])
        .map(|value| if value == 0 { 0o644 } else { value & 0o777 })
        .unwrap_or(0o644)
}

pub(super) fn write_string(
    buffer: &mut [u8],
    value: &str,
    offset: usize,
    length: usize,
) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() > length {
        return Err(format!("plugin archive header value is too long: {value}"));
    }
    buffer[offset..offset + bytes.len()].copy_from_slice(bytes);
    Ok(())
}

pub(super) fn write_octal(buffer: &mut [u8], value: u64, offset: usize, length: usize) {
    let encoded = format!("{value:o}");
    let keep = length - 1;
    let digits = if encoded.len() > keep {
        &encoded[encoded.len() - keep..]
    } else {
        encoded.as_str()
    };
    let padded = format!("{digits:0>keep$}");
    write_string(buffer, &padded, offset, keep).expect("octal fits");
    buffer[offset + length - 1] = 0;
}

pub(super) fn padded(size: u64) -> Result<u64, String> {
    size.checked_add(BLOCK as u64 - 1)
        .map(|value| value / BLOCK as u64 * BLOCK as u64)
        .ok_or_else(|| "invalid plugin sync archive entry size".to_owned())
}
