//! Test-only driver for the shared credential store (docs/specs/rust-mcp-oauth.md): one JSON op per stdin line.
//! `{"op":"save","entries":[[k,v]]}` / `{"op":"load","key":k}` / `{"op":"encrypt","value":v}` /
//! `{"op":"decrypt","value":v}` / `{"op":"secret"}` / `{"op":"burst","prefix":p,"count":n}`.
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use zcode_cli_rust::adapters::{
    credential_cipher::{CredentialCipher, credential_secret},
    credential_store::CredentialStore,
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let store = CredentialStore::new(path, CredentialCipher::from_environment());
    let cipher = CredentialCipher::from_environment();
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let input: Value = serde_json::from_str(&line)?;
        let result: anyhow::Result<Value> = async {
            Ok(match input["op"].as_str().unwrap_or_default() {
                "save" => {
                    let entries = input["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|e| {
                            (
                                e[0].as_str().unwrap().to_owned(),
                                e[1].as_str().unwrap().to_owned(),
                            )
                        })
                        .collect::<Vec<_>>();
                    store.save_many(&entries).await?;
                    json!(null)
                }
                "load" => json!(store.load(input["key"].as_str().unwrap()).await?),
                "encrypt" => json!(cipher.encrypt(input["value"].as_str().unwrap())?),
                "decrypt" => json!(cipher.decrypt(input["value"].as_str().unwrap())?),
                "secret" => json!(credential_secret()),
                // 每次一个 key 的独立 read-modify-write，用于与 Node 交错写入同一文件。
                "burst" => {
                    let prefix = input["prefix"].as_str().unwrap();
                    for i in 0..input["count"].as_u64().unwrap() {
                        store
                            .save_many(&[(format!("{prefix}{i}"), format!("v{i}"))])
                            .await?;
                    }
                    json!(null)
                }
                other => anyhow::bail!("unknown op {other}"),
            })
        }
        .await;
        let reply = match result {
            Ok(value) => json!({"ok":value}),
            Err(error) => json!({"error":error.to_string()}),
        };
        out.write_all(format!("{reply}\n").as_bytes()).await?;
        out.flush().await?;
    }
    Ok(())
}
