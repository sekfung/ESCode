# Rust Host `credential` 域（U2）

> 状态：交付 · 2026-10-15。TS 真相源：`packages/services/src/credential/credentialService.ts`（149 行）、`providers/credentialCipherProvider.ts`。

## 1. 范围（3 个成员）

`credential.load(key) → string | null`、`save(key, value)`、`delete(key)`。Renderer 只经 RPC 读写，不感知存储位置。

## 2. 所有者与格式

唯一事实源：`<dataBaseDir>/.escode/v2/credentials.json`（JSON 对象，值为 `enc:v1:<iv>.<tag>.<cipher>` 的 AES-256-GCM 密文，密钥 = sha256(secret)）。
Host 与 CLI（`apps/escode-cli-rust/crates/host/src/credential_store.rs`，已与 Node 双向互通）共用同一文件，所以不重新实现存储：

```
credential.save/delete ──▶ CredentialStore（CLI host crate）
      file_lock 目录锁（跨进程）→ 读 → 改 → write_private（0600 临时文件 + rename）
credential.load        ──▶ 读 → 解密（非 enc:v1: 前缀的旧明文值原样返回）
```

secret = `ESCODE_CREDENTIAL_SECRET`，否则 `escode-credential-fallback:<platform>:<homedir>:<username>`（与 Node `os.homedir()/userInfo()` 同源规则，CLI 已实现）。
损坏文件：按内容备份后**报错，不当作空表覆盖**（TS 同）。

## 3. 错误形状

解密失败必须带 `code = "ESCODE_CREDENTIAL_DECRYPT_FAILED"` 且 message 以 `凭据解密失败：` 开头（Renderer 的 `isCredentialDecryptError` 据此分流）。
Rust 的解密错误是英文文案，Host 层按原因映射为 TS 文案：密文格式非法 / IV 长度非法 / AuthTag 长度非法 / 密钥不匹配或密文已损坏。

## 4. 登记差异（不修）

1. **空值**：TS 允许 `save(key, "")` 且 Rust 同样允许（CLI 的 `save` 拒绝空值，Host 用新增的 `save_value`）。两边读回都会失败——空串的密文 cipher 段为空，`decrypt` 判"密文格式非法"（TS 原本如此，逐字保持）。
2. **入参校验文案**：键为空时 TS 抛 zod 文案，Rust 抛 `Credential key must not be empty`。
3. **损坏文件文案**：TS `ESCode credentials are corrupt: <file>`；Rust `Shared ESCode credentials are corrupt: <file>. Backup: …`（均带备份证据，不覆盖）。
4. **`onDidMutate`**：TS 的 Host 私有变更通知（telemetry/OAuth 失效联动）不外露；Rust 暂无订阅者，待 oauth 域迁移时在 `CredentialService` 上补。

## 5. 验收

- 双向互通差分（`rustHostCredential.test.ts`）：同一 `ESCODE_CREDENTIAL_SECRET` 与 dataBaseDir 下，TS 服务写 → Rust Host 读；Rust Host 写 → TS 服务读；
  `delete` 互删；明文旧值原样读出；损坏文件两边都报错且不改文件；解密失败的 `code`/前缀一致；`save("k","")` 往返。
- Rust 单测：错误文案映射表；键裁剪。
- 负向验证：把 `delete` 改成不写盘 / 把 secret 改错，对应用例失败。
