//! pty 输出的增量 UTF-8 解码（node-pty `encoding: "utf8"` 走 Node `StringDecoder`）。
//!
//! 一次 `read` 可能把多字节字符切成两半：不完整的尾部留到下一块再解，而不是各自产出 U+FFFD。
//! 非法序列按「最大有效子串」各产出一个 U+FFFD（`from_utf8` 的 `error_len` 与 Node 同口径）。

#[derive(Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let mut out = String::with_capacity(data.len());
        let mut rest: &[u8] = &data;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    out.push_str(&String::from_utf8_lossy(valid));
                    match error.error_len() {
                        Some(length) => {
                            out.push('\u{fffd}');
                            rest = &after[length..];
                        }
                        // 序列在块尾被截断：留待下一块。
                        None => {
                            self.pending = after.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Utf8Decoder;

    #[test]
    fn multibyte_split_across_chunks_decodes_once() {
        let bytes = "中文é🙂".as_bytes();
        for split in 1..bytes.len() {
            let mut decoder = Utf8Decoder::default();
            let joined = decoder.push(&bytes[..split]) + &decoder.push(&bytes[split..]);
            assert_eq!(joined, "中文é🙂", "split={split}");
        }
    }

    #[test]
    fn invalid_bytes_become_one_replacement_each() {
        let mut decoder = Utf8Decoder::default();
        assert_eq!(decoder.push(b"a\xffb\xc3("), "a\u{fffd}b\u{fffd}(");
        assert_eq!(decoder.push(b"\xe4\xb8"), "", "不完整尾部先留着");
        assert_eq!(decoder.push(b"\xad!"), "中!");
    }
}
