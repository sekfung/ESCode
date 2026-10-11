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
    /// guarded 匹配用的单词事实（TS `BashCommandInvocation.words`，不含赋值前缀与重定向；
    /// 管道的 `time` 前缀按 TS 恢复为首词）。偏移为 UTF-16。
    pub words: Vec<crate::guarded::CommandWord>,
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
                // 修复（对齐 unbash，差分语料发现）：语句位置上的分隔符 / 操作符（`; ls`、`ls; ; pwd`）
                // unbash 静默停止解析、保留已得命令且不报错；Rust 原先记解析错误。这类输入 bash 本身
                // 报语法错误、整行不执行，按 TS 停止不会放宽任何实际执行的命令。
                return self.stop();
            }
            self.and_or(if first { None } else { Some("sequence") });
            first = false;
            match self.tokens.get(self.i) {
                Some(Token::Op(";")) | Some(Token::Newline) => self.i += 1,
                // 语句后的 `;;`（`ls ;; pwd`）：unbash 静默停止。
                Some(Token::Op(";;")) => return self.stop(),
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
                // 悬空的 `&&` / `||`（`ls &&`）：unbash 不报错（bash 同样整行不执行）。
                return self.stop();
            }
            self.pipeline(Some(op));
        }
    }

    fn pipeline(&mut self, before: Option<&'static str>) {
        // unbash 把 `time [-p]` 解析为管道前缀而不是命令名，与 TS 保持一致。
        let mut time = None;
        if let Some(Token::Word(w)) = self.tokens.get(self.i)
            && w.value == "time"
            && !w.dynamic
            && matches!(self.tokens.get(self.i + 1), Some(Token::Word(_)))
        {
            time = Some(self.word_fact(&w.clone()));
            self.i += 1;
            if matches!(self.tokens.get(self.i), Some(Token::Word(w)) if w.value == "-p") {
                self.i += 1;
            }
        }
        // 管道取反 `! cmd`：unbash 跳过一个前导 `!`；紧接的第二个 `!` 处于命令位置，停止解析（`! ! ls` 无命令）。
        if self.is_bang() {
            self.i += 1;
            if self.is_bang() {
                return self.stop();
            }
        }
        let first = self.out.commands.len();
        self.command(before);
        // TS：unbash 把 time 关键字提升为 Pipeline.time，collectPipelineCommands 再把它恢复为首个命令的
        // 首词，否则 `time -o file rm -rf x` 的参数会被误当成 executable（guarded 漏判）。
        if let (Some(time), Some(command)) = (time, self.out.commands.get_mut(first)) {
            command.words.insert(0, time);
        }
        while let Some(Token::Op(op @ ("|" | "|&"))) = self.tokens.get(self.i) {
            let op = *op;
            self.i += 1;
            self.skip_newlines();
            // 悬空的 `|`（`ls |`）与管道后的 `!`（`ls | ! rm x`，bash 报语法错误）：unbash 静默停止。
            if self.at_end_of_command() || self.is_bang() {
                return self.stop();
            }
            self.command(Some(op));
        }
    }

    fn at_end_of_command(&self) -> bool {
        matches!(self.tokens.get(self.i), None | Some(Token::Op(..)))
    }

    fn is_bang(&self) -> bool {
        matches!(self.tokens.get(self.i), Some(Token::Word(w)) if w.value == "!" && !w.dynamic)
    }

    /// unbash 在命令位置遇到不能开始命令的 token 时静默停止：保留已解析的命令，不设任何标志。
    fn stop(&mut self) {
        self.i = self.tokens.len();
    }

    fn command(&mut self, before: Option<&'static str>) {
        if self.skip_compound() {
            return;
        }
        match self.tokens.get(self.i) {
            Some(Token::Op("(")) => return self.unsupported_until_separator(),
            // 命令位置上的 `}`（`{ls;}` 拆成 `{ls` `;` `}`）：unbash 静默停止，不记不支持。
            Some(Token::Word(w)) if w.value == "}" && !w.dynamic => return self.stop(),
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
        inv.words = words.iter().map(|w| self.word_fact(w)).collect();
        inv.name = inv.argv.first().cloned().unwrap_or_default();
        inv.command_text = self.src[start..end].to_owned();
        inv.has_dynamic_words = dynamic;
        self.out.has_dynamic_words |= dynamic;
        self.out.has_redirects |= !inv.redirects.is_empty();
        self.out.commands.push(inv);
    }

    fn word_fact(&self, w: &Word) -> crate::guarded::CommandWord {
        let utf16 = |byte: usize| self.src[..byte].encode_utf16().count();
        crate::guarded_word::bash_word(&self.src[w.start..w.end], utf16(w.start), utf16(w.end))
    }

    /// 复合命令（子 shell、`{ }`、`[[ ]]`、if / for / while / until / select / case）：记不支持后跳过整个
    /// 结构（含其后的重定向），之后的独立命令照常收集——unbash 把它们当作局部不支持节点，不丢弃后续
    /// 命令（guarded 语料：`for …; done; rm -rf build` 必须命中）。关键字按非动态词计数，是近似：
    /// 计数失配只会让后续词多收或少收，标志位已记不支持，只读判定不会因此放宽。
    fn skip_compound(&mut self) -> bool {
        let word = |token: Option<&Token>| match token {
            Some(Token::Word(w)) if !w.dynamic => Some(w.value.clone()),
            _ => None,
        };
        let (open, close): (&[&str], &[&str]) = match self.tokens.get(self.i) {
            Some(Token::Op("(")) => (&["("], &[")"]),
            _ => match word(self.tokens.get(self.i)).as_deref() {
                Some("{") => (&["{"], &["}"]),
                Some("[[") => (&["[["], &["]]"]),
                Some("if") => (&["if"], &["fi"]),
                Some("case") => (&["case"], &["esac"]),
                Some("for" | "while" | "until" | "select") => {
                    (&["for", "while", "until", "select"], &["done"])
                }
                _ => return false,
            },
        };
        let is_op = open == ["("];
        let mut depth = 0usize;
        while self.i < self.tokens.len() {
            let text = match &self.tokens[self.i] {
                Token::Op(op) if is_op => Some((*op).to_owned()),
                token if !is_op => word(Some(token)),
                _ => None,
            };
            self.i += 1;
            match text.as_deref() {
                Some(t) if open.contains(&t) => depth += 1,
                Some(t) if close.contains(&t) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        while matches!(self.tokens.get(self.i), Some(Token::Redirect { .. })) {
            self.out.has_redirects = true;
            self.i += 1;
        }
        self.out.has_unsupported_syntax = true;
        true
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
