//! marketplace source 归档导入（TS `pluginSyncService.ts:361-402,566-590`）。

use std::path::Path;

use serde_json::Value;

use super::archive::extract_archive;
use super::marketplace_manifest::{read_manifest, require_string};
use super::{candidates, paths};

pub(super) fn import(
    home: &Path,
    archive: &[u8],
    max_bytes: u64,
    overwrite: bool,
    compare: &crate::skills::Compare,
) -> Result<Value, String> {
    if overwrite {
        return Err("plugin marketplace source overwrite is not supported".to_owned());
    }
    if archive.len() as u64 > max_bytes {
        return Err(format!(
            "plugin marketplace source archive exceeds limit: {}/{max_bytes}",
            archive.len()
        ));
    }
    let temp = home.join(".escode").join("tmp").join(format!(
        "plugin-marketplace-source-{}",
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        extract_archive(archive, &temp, Some(max_bytes))?;
        let metadata = read_metadata(&temp)?;
        let directory_name =
            paths::normalize_relative(&metadata.directory_name, "unsafe plugin archive path")?;
        let extracted =
            paths::resolve_within(&temp, &directory_name, "unsafe plugin archive path")?;
        let manifest = read_manifest(&extracted.join("marketplace.json"))?;
        if manifest.name != metadata.marketplace_id {
            return Err(format!(
                "marketplace source archive id mismatch: {} !== {}",
                manifest.name, metadata.marketplace_id
            ));
        }
        let target_root = candidates::marketplace_root(home);
        let target =
            paths::resolve_within(&target_root, &directory_name, "unsafe plugin archive path")?;
        if target.exists() {
            return Ok(serde_json::json!({
                "marketplaceId": metadata.marketplace_id,
                "path": target.to_string_lossy(),
                "status": "skipped",
            }));
        }
        copy_dir(&extracted, &target)?;
        let _ = compare;
        Ok(serde_json::json!({
            "marketplaceId": metadata.marketplace_id,
            "path": target.to_string_lossy(),
            "status": "synced",
        }))
    })();
    let _ = std::fs::remove_dir_all(&temp);
    result
}

struct ImportMetadata {
    marketplace_id: String,
    directory_name: String,
}

fn read_metadata(root: &Path) -> Result<ImportMetadata, String> {
    let text = std::fs::read_to_string(root.join(super::archive::METADATA_ARCHIVE_PATH))
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let Some(object) = value.as_object() else {
        return Err("invalid plugin marketplace source archive metadata".to_owned());
    };
    let Some(sources) = object.get("marketplaceSources").and_then(Value::as_array) else {
        return Err("invalid plugin marketplace source archive metadata".to_owned());
    };
    if sources.len() != 1 {
        return Err("plugin marketplace source archive must contain one marketplace".to_owned());
    }
    let Some(source) = sources[0].as_object() else {
        return Err("invalid plugin marketplace source metadata".to_owned());
    };
    Ok(ImportMetadata {
        marketplace_id: candidates::normalize_plugin_id(&require_string(
            source.get("marketplaceId"),
            "marketplace source id",
        )?)?,
        directory_name: require_string(
            source.get("directoryName"),
            "marketplace source directory",
        )?,
    })
}

fn copy_dir(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let destination = target.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &destination)?;
        } else {
            std::fs::copy(&path, &destination).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
