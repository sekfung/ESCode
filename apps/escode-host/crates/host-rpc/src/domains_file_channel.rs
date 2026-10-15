//! `file` channel 的方法表（分派即表；未登记方法由 `session::respond` 回显式缺口）。
//! 方法实现见 `domains_file` / `domains_file_preview` / `domains_file_workspace`。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::domains_file::{
    Cache, blocking, check_files_exist, read_file_range, read_text_file, resolve_path, stat,
};
use crate::domains_file_preview::{read_binary_preview, read_media_preview};
use crate::domains_file_readdir::readdir;
use crate::domains_file_workspace::{
    create_default_workspace, create_scratch_workspace, ensure_conversation_workspace,
};
use crate::session::ChannelResult;
use crate::session::{Channel, handler};
use crate::wire::Wire;

/// `file` channel（TS `ServiceChannels.File` = `"file"`）。`home` 与 settings/system 同源
/// （Main 经 `rpcListen` 给出），供 homedir 派生的工作区创建方法用；`data_base_dir` 供
/// 会话工作区（`<dataBaseDir>/.escode/workspace/default`）。
/// 方法表即分派：表里没有的方法（`readdir`/搜索/索引等）由 `respond` 回显式缺口。
pub fn file_channel(home: PathBuf, data_base_dir: Option<PathBuf>) -> Channel {
    let cache = Arc::new(std::sync::Mutex::new(Cache::default()));
    let with_cache = |method: fn(&std::sync::Mutex<Cache>, &Value) -> ChannelResult| {
        let cache = Arc::clone(&cache);
        handler(move |arg: Wire| {
            let cache = Arc::clone(&cache);
            let params = first_object(arg);
            Box::pin(async move { blocking(move || method(&cache, &params)).await })
        })
    };
    let plain = |method: fn(&Value) -> ChannelResult| {
        handler(move |arg: Wire| {
            let params = first_object(arg);
            Box::pin(async move { blocking(move || method(&params)).await })
        })
    };
    let with_home = |method: fn(&std::path::Path, &Value) -> ChannelResult| {
        let home = home.clone();
        handler(move |arg: Wire| {
            let home = home.clone();
            let params = first_object(arg);
            Box::pin(async move { blocking(move || method(&home, &params)).await })
        })
    };
    let default_workspace = {
        let home = home.clone();
        handler(move |_arg: Wire| {
            let home = home.clone();
            Box::pin(async move { blocking(move || create_default_workspace(&home)).await })
        })
    };
    let resolve = handler(move |arg: Wire| {
        let params = first_object(arg);
        Box::pin(async move { resolve_path(&params).await })
    });
    let read_media = handler(move |arg: Wire| {
        let params = first_object(arg);
        Box::pin(async move { blocking(move || read_media_preview(&params)).await })
    });
    let read_binary = handler(move |arg: Wire| {
        let params = first_object(arg);
        Box::pin(async move { blocking(move || read_binary_preview(&params)).await })
    });
    let conversation_workspace = match data_base_dir {
        Some(data_base_dir) => handler(move |_arg: Wire| {
            let data_base_dir = data_base_dir.clone();
            Box::pin(async move {
                blocking(move || ensure_conversation_workspace(&data_base_dir)).await
            })
        }),
        None => handler(move |_arg: Wire| {
            // Main 未提交 dataBaseDir：显式缺口而不是按 HOME 猜路径（Host 不推断业务事实）。
            Box::pin(async move {
                Err((
                    "InvalidRequest".to_owned(),
                    "Host has no dataBaseDir: Main must pass dataBaseDir in rpcListen.".to_owned(),
                ))
            })
        }),
    };
    let mut methods = crate::domains_file_index::index_methods();
    methods.extend([
        ("stat", plain(stat)),
        ("readdir", plain(readdir)),
        ("checkFilesExist", with_cache(check_files_exist)),
        ("resolvePath", resolve),
        ("readFileRange", plain(read_file_range)),
        ("readTextFile", plain(read_text_file)),
        ("readMediaPreview", read_media),
        ("readBinaryPreview", read_binary),
        ("createDefaultWorkspace", default_workspace),
        (
            "createScratchWorkspace",
            with_home(create_scratch_workspace),
        ),
        ("ensureConversationWorkspace", conversation_workspace),
    ]);
    Channel::new("file", methods)
}

/// RPC 实参：`ProxyChannel.toService` 把 JS 实参打包成数组，取第一个并当对象用。
fn first_object(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}
