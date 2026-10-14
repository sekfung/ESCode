//! Guarded 的 CMD 有限入口（TS guarded/cmd.ts）：双引号与 caret；单引号是普通字符，复杂块 / 变量
//! 展开不解释。位置按 UTF-16 偏移计（与 TS 字符串下标一致）。

use crate::guarded::CommandWord;

fn is_delete_builtin(value: &str) -> bool {
    ["rd", "rmdir", "del", "erase"]
        .iter()
        .any(|name| value.eq_ignore_ascii_case(name))
}

struct State {
    commands: Vec<Vec<CommandWord>>,
    words: Vec<CommandWord>,
    value: String,
    /// 当前词起点（字符下标）；`None` 表示尚未开始。
    start: Option<usize>,
    quote: bool,
    dynamic: bool,
    glob: bool,
    skip_target: bool,
    unsupported: bool,
    invalid_segment: bool,
    block_depth: usize,
}

impl State {
    fn flush_word(&mut self, end: usize, offsets: &[usize]) {
        let Some(start) = self.start.take() else {
            return;
        };
        if !self.skip_target {
            self.words.push(CommandWord {
                value: std::mem::take(&mut self.value),
                dynamic: self.dynamic,
                has_unquoted_glob: self.glob,
                start: offsets[start],
                end: offsets[end],
            });
        }
        self.skip_target = false;
        self.value.clear();
        self.dynamic = false;
        self.glob = false;
    }

    fn flush_command(&mut self, end: usize, offsets: &[usize]) {
        self.flush_word(end, offsets);
        let comment = self
            .words
            .first()
            .is_some_and(|word| word.value.eq_ignore_ascii_case("rem") || word.value == "::");
        let words = std::mem::take(&mut self.words);
        if !self.invalid_segment && self.block_depth == 0 && !words.is_empty() && !comment {
            self.commands.push(words);
        }
        self.invalid_segment = self.block_depth > 0;
    }

    /// CMD 内建删除允许 rd/s/q、target/s/q；仅拆这四个内建命令，不能把其它命令的路径或重定向
    /// 目标按开关切开。
    fn split_delete_switch(&mut self, index: usize, offsets: &[usize]) {
        let head = self
            .words
            .first()
            .map(|word| word.value.clone())
            .unwrap_or_else(|| self.value.clone());
        if !self.skip_target && is_delete_builtin(&head) {
            self.flush_word(index, offsets);
            self.start = Some(index);
        }
    }
}

/// TS `analyzeCmdCommand`：返回 `(命令词组, 是否有不支持的片段)`。
pub(crate) fn analyze(source: &str) -> (Vec<Vec<CommandWord>>, bool) {
    let chars: Vec<char> = source.chars().collect();
    let mut offsets = Vec::with_capacity(chars.len() + 1);
    let mut offset = 0;
    for c in &chars {
        offsets.push(offset);
        offset += c.len_utf16();
    }
    offsets.push(offset);
    let mut s = State {
        commands: vec![],
        words: vec![],
        value: String::new(),
        start: None,
        quote: false,
        dynamic: false,
        glob: false,
        skip_target: false,
        unsupported: false,
        invalid_segment: false,
        block_depth: 0,
    };
    let at = |i: usize| chars.get(i).copied();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !s.quote && c == '^' {
            s.start.get_or_insert(i);
            match at(i + 1) {
                None | Some('\r' | '\n') => {
                    s.unsupported = true;
                    s.invalid_segment = true;
                }
                Some(next) => {
                    if next == '/' {
                        s.split_delete_switch(i, &offsets);
                    }
                    s.value.push(next);
                    i += 1;
                }
            }
            i += 1;
            continue;
        }
        if c == '"' {
            s.start.get_or_insert(i);
            s.quote = !s.quote;
            i += 1;
            continue;
        }
        if !s.quote && (c == ' ' || c == '\t') {
            s.flush_word(i, &offsets);
            i += 1;
            continue;
        }
        if !s.quote && matches!(c, '&' | '|' | '\r' | '\n') {
            s.flush_command(i, &offsets);
            if at(i + 1) == Some(c) {
                i += 1;
            }
            i += 1;
            continue;
        }
        if !s.quote && (c == '<' || c == '>') {
            // 2>&1 等 fd 重定向不是命令分隔符，也不是 argv。
            if s.start.is_some()
                && !s.value.is_empty()
                && s.value.chars().all(|d| d.is_ascii_digit())
            {
                s.value.clear();
                s.start = None;
            }
            s.flush_word(i, &offsets);
            if at(i + 1) == Some(c) {
                i += 1;
            }
            if at(i + 1) == Some('&') && at(i + 2).is_some_and(|d| d.is_ascii_digit() || d == '-') {
                i += 2;
            } else {
                s.skip_target = true;
            }
            i += 1;
            continue;
        }
        if !s.quote && (c == '(' || c == ')') {
            s.unsupported = true;
            s.invalid_segment = true;
            s.block_depth = if c == '(' {
                s.block_depth + 1
            } else {
                s.block_depth.saturating_sub(1)
            };
        }
        if !s.quote && c == '/' {
            s.split_delete_switch(i, &offsets);
        }
        let start = *s.start.get_or_insert(i);
        if c == '%' || c == '!' {
            s.dynamic = true;
        }
        if !s.quote && matches!(c, '*' | '?' | '[') {
            s.glob = true;
        }
        s.value.push(c);
        // 重定向目标 rem 是文件名；只有命令位置的 REM 才能吞掉剩余注释文本。
        if !s.quote && !s.skip_target && s.words.is_empty() && starts_comment(&chars[start..]) {
            s.value.clear();
            s.start = None;
            i = chars[i..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(chars.len(), |p| i + p);
        }
        i += 1;
    }
    // 只有重定向符后没有开始任何目标词才是缺目标；目标词已在累积时按重定向目标正常丢弃。
    if s.quote || (s.skip_target && s.start.is_none()) {
        s.unsupported = true;
        s.invalid_segment = true;
    }
    s.flush_command(chars.len(), &offsets);
    (s.commands, s.unsupported)
}

/// TS `/^(?:rem\s|::)/i`（`\s` 取 JS 空白集合的 ASCII 与常见 Unicode 部分）。
fn starts_comment(rest: &[char]) -> bool {
    if rest.len() >= 2 && rest[0] == ':' && rest[1] == ':' {
        return true;
    }
    rest.len() >= 4
        && rest[..3]
            .iter()
            .collect::<String>()
            .eq_ignore_ascii_case("rem")
        && (rest[3].is_whitespace() || rest[3] == '\u{feff}')
}
