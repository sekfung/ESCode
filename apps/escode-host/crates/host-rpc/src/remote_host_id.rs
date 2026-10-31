//! `remote:server:<sourceServerId>` host 身份的构造/解析。
//!
//! 与 `packages/shared/src/remote-workspace-identity.ts` 同口径：serverId 先转小写，
//! 非 `[a-z0-9._-]` 字符折成 `-`；host 身份 = identity 去掉 `:<workspacePath>` 后的前缀。

const REMOTE_IDENTITY_PREFIX: &str = "remote:";

pub(crate) fn normalize_server_id(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

pub(crate) fn host_id_for_source(source_server_id: &str) -> String {
    format!(
        "{REMOTE_IDENTITY_PREFIX}server:{}",
        normalize_server_id(source_server_id)
    )
}

/// 只接受 `remote:server:<id>[:<path>]`；返回稳定 Source id。
pub(crate) fn source_id_from_host_id(host_id: &str) -> Option<&str> {
    let rest = host_id.strip_prefix("remote:server:")?;
    let (source_id, _) = rest.split_once(':').unwrap_or((rest, ""));
    (!source_id.is_empty()).then_some(source_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_parses_server_host_ids() {
        assert_eq!(host_id_for_source("AbC"), "remote:server:abc");
        assert_eq!(
            source_id_from_host_id("remote:server:abc:/srv/w"),
            Some("abc")
        );
        assert_eq!(source_id_from_host_id("remote:server:abc"), Some("abc"));
        assert_eq!(source_id_from_host_id("remote:ssh:host:22:root:/srv"), None);
        assert_eq!(source_id_from_host_id("local"), None);
    }
}
