//! Bash 词法的引号与展开：单/双引号、ANSI-C、`$` 展开、反引号与成对区段跳过。
use crate::bash_lex::{Lexer, Word};

impl Lexer<'_> {
    pub(crate) fn quoted_until(&mut self, close: char, w: &mut Word, escapes: bool) {
        while let Some(c) = self.peek(0) {
            self.i += 1;
            if escapes && c == '\\' {
                if let Some(n) = self.peek(0) {
                    self.i += 1;
                    w.value.push(n);
                }
                continue;
            }
            if c == close {
                return;
            }
            w.value.push(c);
        }
        self.error = true;
    }

    pub(crate) fn double_quoted(&mut self, w: &mut Word) {
        while let Some(c) = self.peek(0) {
            match c {
                '"' => {
                    self.i += 1;
                    return;
                }
                '\\' => match self.peek(1) {
                    Some(n @ ('$' | '`' | '"' | '\\')) => {
                        w.value.push(n);
                        self.i += 2;
                    }
                    Some('\n') => self.i += 2,
                    _ => {
                        w.value.push('\\');
                        self.i += 1;
                    }
                },
                '$' => self.dollar(w),
                '`' => {
                    w.dynamic = true;
                    self.i += 1;
                    self.quoted_until('`', &mut Word::default(), true);
                }
                _ => {
                    w.value.push(c);
                    self.i += 1;
                }
            }
        }
        self.error = true;
    }

    pub(crate) fn dollar(&mut self, w: &mut Word) {
        match self.peek(1) {
            Some('\'') => {
                // $'..'：ANSI-C 引号，静态。
                self.i += 2;
                self.quoted_until('\'', w, true);
            }
            Some('"') => {
                self.i += 2;
                self.double_quoted(w);
            }
            Some('{') => {
                w.dynamic = true;
                w.value.push_str(&self.skip_group('{', '}', 1));
            }
            Some('(') => {
                w.dynamic = true;
                w.value.push_str(&self.skip_group('(', ')', 1));
            }
            Some(c) if c.is_ascii_alphanumeric() || "_@*#?$!-".contains(c) => {
                w.dynamic = true;
                let start = self.pos();
                self.i += 2;
                if c.is_ascii_alphabetic() || c == '_' {
                    while self
                        .peek(0)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        self.i += 1;
                    }
                }
                w.value.push_str(&self.src[start..self.pos()]);
            }
            _ => {
                w.value.push('$');
                self.i += 1;
            }
        }
    }

    /// 从 `self.i + offset` 处的开括号起跳过配对区段（考虑嵌套与引号），返回原文。
    pub(crate) fn skip_group(&mut self, open: char, close: char, offset: usize) -> String {
        let start = self.pos();
        self.i += offset;
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            self.i += 1;
            match c {
                '\\' => self.i += 1,
                '\'' => self.quoted_until('\'', &mut Word::default(), false),
                '"' => self.double_quoted(&mut Word::default()),
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return self.src[start..self.pos().min(self.src.len())].to_owned();
                    }
                }
                _ => {}
            }
        }
        self.error = true;
        self.src[start..].to_owned()
    }
}
