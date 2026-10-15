use super::*;

fn error_of(text: &str) -> CredentialError {
    map_error(&anyhow::anyhow!(text.to_owned()))
}

#[test]
fn decrypt_errors_use_the_ts_wording_and_code() {
    for (english, reason) in [
        (
            "Credential decrypt failed: invalid ciphertext format",
            "密文格式非法",
        ),
        (
            "Credential decrypt failed: invalid IV length",
            "IV 长度非法",
        ),
        (
            "Credential decrypt failed: invalid auth tag length",
            "AuthTag 长度非法",
        ),
        (
            "Credential decrypt failed: key mismatch or corrupted ciphertext",
            "密钥不匹配或密文已损坏",
        ),
    ] {
        let error = error_of(english);
        assert_eq!(error.message, format!("凭据解密失败：{reason}"));
        assert_eq!(error.code, Some("ESCODE_CREDENTIAL_DECRYPT_FAILED"));
    }
    assert_eq!(
        error_of("Invalid symbol 33, offset 4.").code,
        Some(DECRYPT_ERROR_CODE)
    );
    assert_eq!(error_of("disk full").code, None);
}

#[tokio::test]
async fn round_trips_empty_values_and_unconditional_delete() {
    let dir = tempfile::tempdir().expect("dir");
    let service = CredentialService::new(CredentialService::path(dir.path()), "secret");
    assert_eq!(service.load("k").await.expect("load"), None);
    service.save(" k ", "").await.expect("save empty");
    // 空值能写入（键被裁剪），但密文的 cipher 段为空，读取时与 TS 一样判“密文格式非法”（spec §4-1）。
    let error = service.load("k").await.expect_err("empty ciphertext part");
    assert_eq!(error.message, "凭据解密失败：密文格式非法");
    service.save("k", "v").await.expect("overwrite");
    assert_eq!(service.load("k").await.expect("load").as_deref(), Some("v"));
    service.delete("k").await.expect("delete");
    service.delete("k").await.expect("delete again is a no-op");
    assert_eq!(service.load("k").await.expect("load"), None);
    assert!(service.save("  ", "v").await.is_err());
    let wrong = CredentialService::new(CredentialService::path(dir.path()), "other");
    service.save("k", "v").await.expect("save");
    let error = wrong.load("k").await.expect_err("wrong secret");
    assert_eq!(error.code, Some(DECRYPT_ERROR_CODE));
}
