//! 与 Node 共用的凭据加密（docs/specs/rust-mcp-oauth.md，对齐 TS `adapters/src/auth/credential-cipher.ts`）：
//! AES-256-GCM，值形如 `enc:v1:<iv>.<tag>.<cipher>`（base64url），密钥 = sha256(secret)。
//! secret 取 `ZCODE_CREDENTIAL_SECRET`，否则 `zcode-credential-fallback:<platform>:<homedir>:<username>`，
//! 其中 homedir/username 与 Node `os.homedir()` / `os.userInfo().username` 同源（libuv 规则）。
use anyhow::{Result, bail};
use base64::Engine as _;
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use sha2::{Digest, Sha256};

const PREFIX: &str = "enc:v1:";
const IV_BYTES: usize = 12;
const TAG_BYTES: usize = 16;

pub struct CredentialCipher {
    key: LessSafeKey,
}

/// 密码学安全随机字节（OAuth state、PKCE verifier、凭据 generation）。
pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("system randomness unavailable");
    bytes
}

/// Node `os.platform()`。
pub fn node_platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// libuv `uv_os_homedir`：Windows 先看 USERPROFILE，POSIX 先看 HOME，再回落到账户数据库。
pub fn node_homedir() -> String {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    if let Some(value) = std::env::var_os(variable).filter(|v| !v.is_empty()) {
        return value.to_string_lossy().into_owned();
    }
    passwd().map(|(_, home)| home).unwrap_or_default()
}

/// Node `os.userInfo().username`（不读环境变量）；取不到时 TS 用 `unknown`。
pub fn node_username() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;
        let mut buffer = vec![0u16; 257];
        let mut size = buffer.len() as u32;
        // SAFETY: buffer 长度与 size 一致；成功时 size 含结尾 NUL。
        if unsafe { GetUserNameW(buffer.as_mut_ptr(), &mut size) } != 0 && size > 0 {
            return String::from_utf16_lossy(&buffer[..size as usize - 1]);
        }
        "unknown".into()
    }
    #[cfg(not(windows))]
    {
        passwd()
            .map(|(name, _)| name)
            .unwrap_or_else(|| "unknown".into())
    }
}

#[cfg(windows)]
fn passwd() -> Option<(String, String)> {
    None
}
#[cfg(not(windows))]
fn passwd() -> Option<(String, String)> {
    use std::ffi::CStr;
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: 缓冲区与 entry 在调用期间有效；成功时 result 指向 entry。
    let code = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if code != 0 || result.is_null() {
        return None;
    }
    // SAFETY: getpwuid_r 成功时字段指向 buffer 内的 NUL 结尾字符串。
    unsafe {
        Some((
            CStr::from_ptr(entry.pw_name).to_string_lossy().into_owned(),
            CStr::from_ptr(entry.pw_dir).to_string_lossy().into_owned(),
        ))
    }
}

/// TS `resolveCredentialSecret`。
pub fn credential_secret() -> String {
    if let Some(secret) = std::env::var("ZCODE_CREDENTIAL_SECRET")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        return secret;
    }
    format!(
        "zcode-credential-fallback:{}:{}:{}",
        node_platform(),
        node_homedir(),
        node_username()
    )
}

impl CredentialCipher {
    pub fn new(secret: &str) -> Self {
        let digest = Sha256::digest(secret.as_bytes());
        let key = UnboundKey::new(&AES_256_GCM, &digest).expect("32-byte key");
        Self {
            key: LessSafeKey::new(key),
        }
    }
    pub fn from_environment() -> Self {
        Self::new(&credential_secret())
    }
    /// 非 `enc:v1:` 的值原样返回（TS 兼容明文旧值）。
    pub fn decrypt(&self, value: &str) -> Result<String> {
        let Some(payload) = value.strip_prefix(PREFIX) else {
            return Ok(value.to_owned());
        };
        let parts: Vec<&str> = payload.split('.').collect();
        let [iv, tag, cipher] = parts.as_slice() else {
            bail!("Credential decrypt failed: invalid ciphertext format");
        };
        if iv.is_empty() || tag.is_empty() || cipher.is_empty() {
            bail!("Credential decrypt failed: invalid ciphertext format");
        }
        let decode = |part: &str| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(part.trim_end_matches('='))
        };
        let (iv, tag, cipher) = (decode(iv)?, decode(tag)?, decode(cipher)?);
        if iv.len() != IV_BYTES {
            bail!("Credential decrypt failed: invalid IV length");
        }
        if tag.len() != TAG_BYTES {
            bail!("Credential decrypt failed: invalid auth tag length");
        }
        let nonce = Nonce::try_assume_unique_for_key(&iv)
            .map_err(|_| anyhow::anyhow!("Credential decrypt failed: invalid IV length"))?;
        let mut sealed = cipher;
        sealed.extend_from_slice(&tag);
        let plain = self
            .key
            .open_in_place(nonce, Aad::empty(), &mut sealed)
            .map_err(|_| {
                anyhow::anyhow!("Credential decrypt failed: key mismatch or corrupted ciphertext")
            })?;
        Ok(String::from_utf8_lossy(plain).into_owned())
    }
    pub fn encrypt(&self, value: &str) -> Result<String> {
        let mut iv = [0u8; IV_BYTES];
        SystemRandom::new()
            .fill(&mut iv)
            .map_err(|_| anyhow::anyhow!("Credential encrypt failed: no randomness"))?;
        let mut data = value.as_bytes().to_vec();
        let tag = self
            .key
            .seal_in_place_separate_tag(Nonce::assume_unique_for_key(iv), Aad::empty(), &mut data)
            .map_err(|_| anyhow::anyhow!("Credential encrypt failed"))?;
        let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        Ok(format!(
            "{PREFIX}{}.{}.{}",
            encode(&iv),
            encode(tag.as_ref()),
            encode(&data)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::CredentialCipher;

    #[test]
    fn round_trips_and_rejects_foreign_keys() {
        let cipher = CredentialCipher::new("secret-a");
        let sealed = cipher.encrypt("token-值").unwrap();
        assert!(sealed.starts_with("enc:v1:"));
        assert_eq!(cipher.decrypt(&sealed).unwrap(), "token-值");
        assert_eq!(cipher.decrypt("plain").unwrap(), "plain");
        assert!(CredentialCipher::new("secret-b").decrypt(&sealed).is_err());
    }
}
