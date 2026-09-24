//! Bash 解析：对应 TS `bash-command-parser.ts::analyzeBashCommand` 的输出结构。
//! 只接受「语句 → and/or → 管道 → 简单命令」；子 shell、控制结构、函数等记为不支持，
//! 后台 `&` 记为不支持但仍收集命令（与 TS 一致）。任何不确定的输入都走不支持/错误方向，不会放宽只读判定。
use crate::bash_lex::{Token, Word, lex};

const MAX_BASH_PARSE_LENGTH: usize = 10_000;
const RESERVED: [&str; 20] = [
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "select", "coproc", "[[", "]]", "{", "}", "!",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Redirect {
    pub fd: Option<u32>,
    pub op: String,
    pub target: String,
}

#[derive(Clone, Debug, Default)]
pub struct Invocation {
    pub argv: Vec<String>,
    pub command_text: String,
    pub env: Vec<(String, String)>,
    pub has_dynamic_words: bool,
    pub name: String,
    pub operator_before: Option<&'static str>,
    pub redirects: Vec<Redirect>,
}

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub commands: Vec<Invocation>,
    pub has_dynamic_words: bool,
    pub has_parse_errors: bool,
    pub has_redirects: bool,
    pub has_unsupported_syntax: bool,
}

impl Analysis {
    /// TS `isBashCommandPermissionSafe`。
    pub fn permission_safe(&self) -> bool {
        !self.has_parse_errors && !self.has_unsupported_syntax && !self.has_dynamic_words
    }
}

pub fn analyze(src: &str) -> Analysis {
    let mut analysis = Analysis::default();
    if src.trim().is_empty() {
        return analysis;
    }
    if src.len() > MAX_BASH_PARSE_LENGTH {
        analysis.has_parse_errors = true;
        return analysis;
    }
    let lexed = lex(src);
    analysis.has_parse_errors = lexed.error;
    let mut p = Parser {
        src,
        tokens: lexed.tokens,
        i: 0,
        out: analysis,
    };
    p.list();
    p.out
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    i: usize,
    out: Analysis,
}

impl Parser<'_> {
    fn skip_newlines(&mut self) {
        while matches!(self.tokens.get(self.i), Some(Token::Newline)) {
            self.i += 1;
        }
    }

    /// 语句序列：`;`、换行分隔（operatorBefore = "sequence"），`&` 标记后台。
    fn list(&mut self) {
        let mut first = true;
        loop {
            self.skip_newlines();
            if self.i >= self.tokens.len() {
                return;
            }
            if matches!(
                self.tokens[self.i],
                Token::Op(";" | "&" | ";;" | "|" | "|&" | "&&" | "||")
            ) {
                self.out.has_parse_errors = true;
                self.i += 1;
                continue;
            }
            self.and_or(if first { None } else { Some("sequence") });
            first = false;
            match self.tokens.get(self.i) {
                Some(Token::Op(";")) | Some(Token::Newline) => self.i += 1,
                Some(Token::Op("&")) => {
                    self.out.has_unsupported_syntax = true;
                    self.i += 1;
                }
                None => return,
                Some(_) => {
                    // 词法层面的意外 token（如孤立的右括号）。
                    self.out.has_parse_errors = true;
                    self.i += 1;
                }
            }
        }
    }

    fn and_or(&mut self, before: Option<&'static str>) {
        self.pipeline(before);
        while let Some(Token::Op(op @ ("&&" | "||"))) = self.tokens.get(self.i) {
            let op = *op;
            self.i += 1;
            self.skip_newlines();
            if self.at_end_of_command() {
                self.out.has_parse_errors = true;
                return;
            }
            self.pipeline(Some(op));
        }
    }

    fn pipeline(&mut self, before: Option<&'static str>) {
        // unbash 把 `time [-p]` 解析为管道前缀而不是命令名，与 TS 保持一致。
        if matches!(self.tokens.get(self.i), Some(Token::Word(w)) if w.value == "time" && !w.dynamic)
            && matches!(self.tokens.get(self.i + 1), Some(Token::Word(_)))
        {
            self.i += 1;
            if matches!(self.tokens.get(self.i), Some(Token::Word(w)) if w.value == "-p") {
                self.i += 1;
            }
        }
        self.command(before);
        while let Some(Token::Op(op @ ("|" | "|&"))) = self.tokens.get(self.i) {
            let op = *op;
            self.i += 1;
            self.skip_newlines();
            if self.at_end_of_command() {
                self.out.has_parse_errors = true;
                return;
            }
            self.command(Some(op));
        }
    }

    fn at_end_of_command(&self) -> bool {
        matches!(self.tokens.get(self.i), None | Some(Token::Op(..)))
    }

    fn command(&mut self, before: Option<&'static str>) {
        match self.tokens.get(self.i) {
            Some(Token::Op("(")) => return self.unsupported_until_separator(),
            Some(Token::Word(w)) if RESERVED.contains(&w.value.as_str()) && !w.dynamic => {
                return self.unsupported_until_separator();
            }
            _ => {}
        }
        let mut inv = Invocation {
            operator_before: before,
            ..Default::default()
        };
        let (mut start, mut end) = (usize::MAX, 0);
        let mut words: Vec<Word> = vec![];
        let mut dynamic = false;
        loop {
            match self.tokens.get(self.i).cloned() {
                Some(Token::Word(w)) => {
                    start = start.min(w.start);
                    end = end.max(w.end);
                    self.i += 1;
                    if words.is_empty() && is_assignment(&w) {
                        let (name, value) = w.value.split_once('=').unwrap();
                        let name = name.trim_end_matches('+');
                        dynamic |= w.dynamic;
                        inv.env.push((name.to_owned(), value.to_owned()));
                        continue;
                    }
                    dynamic |= w.dynamic;
                    words.push(w);
                }
                Some(Token::Redirect {
                    fd,
                    op,
                    target,
                    body_dynamic,
                    start: s,
                }) => {
                    start = start.min(s);
                    end = end.max(target.end);
                    self.i += 1;
                    dynamic |= target.dynamic || body_dynamic;
                    inv.redirects.push(Redirect {
                        fd,
                        op: op.to_owned(),
                        target: target.value,
                    });
                }
                Some(Token::Op("(")) => {
                    // `name (` —— 函数定义等，不在子集内。
                    self.out.has_unsupported_syntax = true;
                    return self.unsupported_until_separator();
                }
                _ => break,
            }
        }
        if start == usize::MAX {
            self.out.has_parse_errors = true;
            return;
        }
        inv.argv = words.iter().map(|w| w.value.clone()).collect();
        inv.name = inv.argv.first().cloned().unwrap_or_default();
        inv.command_text = self.src[start..end].to_owned();
        inv.has_dynamic_words = dynamic;
        self.out.has_dynamic_words |= dynamic;
        self.out.has_redirects |= !inv.redirects.is_empty();
        self.out.commands.push(inv);
    }

    /// 不支持的结构：记标志并跳到语句结束（权限上已判为非只读，无需精确解析其内部）。
    fn unsupported_until_separator(&mut self) {
        self.out.has_unsupported_syntax = true;
        self.i = self.tokens.len();
    }
}

fn is_assignment(w: &Word) -> bool {
    let Some(eq) = w.raw_prefix.find('=') else {
        return false;
    };
    let name = w.raw_prefix[..eq].trim_end_matches('+');
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
