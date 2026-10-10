//! Bash git 规则：对应 TS `bash-readonly-policy-argv-git.ts`（全局选项归一与子命令策略匹配）。
use crate::bash_policy::tables;

pub(crate) fn git_dangerous_word(w: &str) -> bool {
    let attached = ["-c", "-C"].iter().any(|f| {
        w.len() > f.len() && w.starts_with(f) && (*f == "-C" || w.as_bytes()[f.len()] != b'-')
    });
    let t = tables();
    attached
        || t.git_dangerous.iter().any(|f| {
            w == f
                || w.strip_prefix(f.as_str())
                    .is_some_and(|r| r.starts_with('='))
        })
}

pub(crate) fn git_readonly(argv: &[String]) -> bool {
    let t = tables();
    let mut normalized = vec!["git".to_owned()];
    let mut i = 1;
    let mut found = false;
    while i < argv.len() {
        let w = &argv[i];
        if w.is_empty() || t.git_no_value.contains(w) {
            i += 1;
            continue;
        }
        if git_dangerous_word(w) {
            return false;
        }
        if t.git_value.contains(w) {
            i += 1;
            if argv.get(i).is_none_or(|v| v.is_empty()) {
                return false;
            }
            i += 1;
            continue;
        }
        if w.starts_with('-') {
            return false;
        }
        normalized.extend_from_slice(&argv[i..]);
        found = true;
        break;
    }
    if !found {
        return false;
    }
    for (prefix, p) in &t.git {
        let words: Vec<&str> = prefix.split(' ').collect();
        if !words
            .iter()
            .enumerate()
            .all(|(i, w)| normalized.get(i).is_some_and(|a| a == w))
        {
            continue;
        }
        if p.callback.as_deref().is_some_and(|cb| {
            crate::bash_callbacks::dangerous(cb, prefix, &normalized[words.len()..])
        }) {
            return false;
        }
        return crate::bash_policy_argv::allowed_by_policy(&normalized, p, "git", words.len());
    }
    false
}
