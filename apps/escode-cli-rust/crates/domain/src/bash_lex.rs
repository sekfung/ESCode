//! Bash 词法：只覆盖 TS `bash-command-parser.ts` 从 unbash 消费的子集（词的去引号值、动态部件、
//! 运算符、重定向与 heredoc）。超出子集的结构由 bash_parse 标记为不支持，权限上一律不视为只读。

#[derive(Clone, Debug, Default)]
pub(crate) struct Word {
    pub value: String,
    pub dynamic: bool,
    /// 未加引号的字面前缀（用于识别 `NAME=` 赋值前缀）。
    pub raw_prefix: String,
    /// 含引号部件（单/双引号、$'..'、$"..."）。unbash 对纯字面词不拆部件，value 直接取原文（保留反斜杠）。
    pub quoted: bool,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug)]
pub(crate) enum Token {
    Word(Word),
    Op(&'static str),
    Redirect {
        fd: Option<u32>,
        op: &'static str,
        target: Word,
        body_dynamic: bool,
        start: usize,
    },
    Newline,
}

pub(crate) struct Lexed {
    pub tokens: Vec<Token>,
    pub error: bool,
}

const OPS: [&str; 9] = ["&&", "||", "|&", ";;", "|", ";", "&", "(", ")"];
const REDIRECTS: [&str; 12] = [
    "&>>", "<<<", "<<-", "&>", "<<", "<&", "<>", ">>", ">&", ">|", "<", ">",
];

pub(crate) fn lex(src: &str) -> Lexed {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut lx = Lexer {
        src,
        chars,
        i: 0,
        tokens: vec![],
        error: false,
        heredocs: vec![],
    };
    lx.run();
    Lexed {
        tokens: lx.tokens,
        error: lx.error,
    }
}

pub(crate) struct Lexer<'a> {
    pub(crate) src: &'a str,
    pub(crate) chars: Vec<(usize, char)>,
    pub(crate) i: usize,
    pub(crate) tokens: Vec<Token>,
    pub(crate) error: bool,
    /// 待读取正文的 heredoc：(token 下标, 分隔符, 去前导 tab, 分隔符是否带引号)。
    heredocs: Vec<(usize, String, bool, bool)>,
}

impl Lexer<'_> {
    pub(crate) fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).map(|c| c.1)
    }
    pub(crate) fn pos(&self) -> usize {
        self.chars.get(self.i).map_or(self.src.len(), |c| c.0)
    }
    fn starts(&self, s: &str) -> bool {
        self.src[self.pos()..].starts_with(s)
    }

    fn run(&mut self) {
        while self.i < self.chars.len() {
            let c = self.peek(0).unwrap();
            if c == ' ' || c == '\t' {
                self.i += 1;
            } else if c == '\\' && self.peek(1) == Some('\n') {
                self.i += 2;
            } else if c == '#' {
                while self.peek(0).is_some_and(|c| c != '\n') {
                    self.i += 1;
                }
            } else if c == '\n' {
                self.i += 1;
                self.tokens.push(Token::Newline);
                self.read_heredoc_bodies();
            } else if (c == '<' || c == '>') && self.peek(1) == Some('(') {
                let w = self.word();
                self.tokens.push(Token::Word(w));
            } else if let Some(op) = self.redirect_op() {
                self.redirect(None, op);
            } else if c.is_ascii_digit() && self.fd_redirect() {
            } else if let Some(op) = OPS.iter().find(|op| self.starts(op)) {
                self.i += op.chars().count();
                self.tokens.push(Token::Op(op));
            } else {
                let w = self.word();
                self.tokens.push(Token::Word(w));
            }
        }
        if !self.heredocs.is_empty() {
            // 缺少结束行的 heredoc 与 unbash 一样按解析错误处理。
            self.error = true;
        }
    }

    fn redirect_op(&self) -> Option<&'static str> {
        REDIRECTS.iter().copied().find(|op| self.starts(op))
    }

    fn fd_redirect(&mut self) -> bool {
        let save = self.i;
        let mut n = 0u32;
        while let Some(d) = self.peek(0).and_then(|c| c.to_digit(10)) {
            n = n.saturating_mul(10).saturating_add(d);
            self.i += 1;
        }
        match self.redirect_op().filter(|op| !op.starts_with('&')) {
            Some(op) => {
                self.redirect(Some(n), op);
                true
            }
            None => {
                self.i = save;
                false
            }
        }
    }

    fn redirect(&mut self, fd: Option<u32>, op: &'static str) {
        let start = self.pos();
        self.i += op.chars().count();
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.i += 1;
        }
        if self.peek(0).is_none_or(|c| "\n;&|()<>".contains(c)) {
            self.error = true;
            return;
        }
        let target = self.word();
        if op == "<<" || op == "<<-" {
            let quoted = self.src[target.start..target.end].contains(['\'', '"', '\\']);
            self.heredocs
                .push((self.tokens.len(), target.value.clone(), op == "<<-", quoted));
        }
        self.tokens.push(Token::Redirect {
            fd,
            op,
            target,
            body_dynamic: false,
            start,
        });
    }

    fn read_heredoc_bodies(&mut self) {
        for (index, delimiter, strip, quoted) in std::mem::take(&mut self.heredocs) {
            let mut dynamic = false;
            let mut closed = false;
            while self.i < self.chars.len() {
                let start = self.pos();
                let end = self.src[start..]
                    .find('\n')
                    .map_or(self.src.len(), |n| start + n);
                let line = &self.src[start..end];
                let cmp = if strip {
                    line.trim_start_matches('\t')
                } else {
                    line
                };
                self.i = self
                    .chars
                    .iter()
                    .position(|c| c.0 > end)
                    .unwrap_or(self.chars.len());
                if cmp == delimiter {
                    closed = true;
                    break;
                }
                if !quoted && (line.contains('$') || line.contains('`')) {
                    dynamic = true;
                }
            }
            if !closed {
                self.error = true;
            }
            if let Some(Token::Redirect { body_dynamic, .. }) = self.tokens.get_mut(index) {
                *body_dynamic = dynamic;
            }
        }
    }

    /// 读一个词直到未加引号的空白或运算符字符。
    fn word(&mut self) -> Word {
        let start = self.pos();
        let mut w = Word {
            start,
            ..Default::default()
        };
        let mut literal_prefix = true;
        let mut brace_depth = 0usize;
        let mut brace_expansion = false;
        let mut first = true;
        while let Some(c) = self.peek(0) {
            if !first && (c == '<' || c == '>') && self.peek(1) == Some('(') {
                // 词内进程替换同样是动态执行。
                w.dynamic = true;
                self.skip_group('(', ')', 1);
                continue;
            }
            if first && (c == '<' || c == '>') && self.peek(1) == Some('(') {
                w.dynamic = true;
                self.skip_group('(', ')', 1);
                first = false;
                continue;
            }
            first = false;
            match c {
                ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | ')' => break,
                '(' => {
                    let prev = w.value.chars().last();
                    if matches!(prev, Some('?' | '*' | '+' | '@' | '!')) && !w.value.is_empty() {
                        w.dynamic = true; // extglob
                        self.skip_group('(', ')', 0);
                        continue;
                    }
                    break;
                }
                '\\' => {
                    literal_prefix = false;
                    match self.peek(1) {
                        Some('\n') => self.i += 2,
                        Some(n) => {
                            w.value.push(n);
                            self.i += 2;
                        }
                        None => {
                            w.value.push('\\');
                            self.i += 1;
                        }
                    }
                }
                '\'' => {
                    literal_prefix = false;
                    w.quoted = true;
                    self.i += 1;
                    self.quoted_until('\'', &mut w, false);
                }
                '"' => {
                    literal_prefix = false;
                    w.quoted = true;
                    self.i += 1;
                    self.double_quoted(&mut w);
                }
                '$' => {
                    literal_prefix = false;
                    self.dollar(&mut w);
                }
                '`' => {
                    literal_prefix = false;
                    w.dynamic = true;
                    self.i += 1;
                    self.quoted_until('`', &mut Word::default(), true);
                }
                _ => {
                    if c == '{' {
                        brace_depth += 1;
                    } else if c == '}' && brace_depth > 0 {
                        brace_depth -= 1;
                    } else if brace_depth > 0
                        && (c == ',' || (c == '.' && self.peek(1) == Some('.')))
                    {
                        brace_expansion = true;
                    }
                    if literal_prefix {
                        w.raw_prefix.push(c);
                    }
                    w.value.push(c);
                    self.i += 1;
                }
            }
        }
        if brace_expansion {
            w.dynamic = true;
        }
        w.end = self.pos();
        if !w.quoted && !w.dynamic {
            // 与 unbash 一致：纯字面词的 value 即源文本（保留反斜杠），因此双反斜杠开头的 UNC 路径仍能被识别。
            w.value = self.src[w.start..w.end].to_owned();
        }
        w
    }
}
