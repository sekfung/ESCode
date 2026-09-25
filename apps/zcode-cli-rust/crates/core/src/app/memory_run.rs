//! 运行侧的项目记忆解析（docs/specs/rust-project-memory.md）：向会话 owner 询问 Host 开关与缓存，
//! 首次启用时按配置解析记忆根并回报 owner 缓存；会话内后续轮次复用同一份（与 TS 上下文初始化一致）。
use crate::contract::{Event, EventSink, ProjectMemory, ToolPort};
use anyhow::Result;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) async fn resolve(
    tools: &dyn ToolPort,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<Option<ProjectMemory>> {
    let (reply, receipt) = oneshot::channel();
    if sink.send(Event::MemoryPreference { reply }).await.is_err() {
        return Ok(None);
    }
    // Host 超时或 owner 放弃时按关闭处理（TS 偏好请求失败的兼容回退同为 memoryEnabled=false）。
    let (enabled, cached) = tokio::select! {biased;
        _ = cancel.cancelled() => anyhow::bail!("Cancelled"),
        answer = tokio::time::timeout(Duration::from_secs(15), receipt) => {
            answer.ok().and_then(Result::ok).unwrap_or((false, None))
        }
    };
    if !enabled {
        return Ok(None);
    }
    let session = sink.session_id.as_str();
    if let Some(memory) = cached {
        // 工具侧上下文可能随会话驱逐清空；每轮重申（不再标记索引已读）。
        tools
            .memory_context(session, &memory.root, session, None, None)
            .await;
        return Ok(Some(memory));
    }
    let memory = tools.project_memory().await;
    if let Some(memory) = &memory {
        // TS 加载非空索引时把 MEMORY.md 记入 readFileState，主会话可直接 Edit 索引。
        let seeded = memory
            .index
            .as_deref()
            .is_some_and(|index| !crate::domain::memory::format_project_index(index).is_empty());
        tools
            .memory_context(
                session,
                &memory.root,
                session,
                None,
                seeded.then_some(memory.index_path.as_str()),
            )
            .await;
        sink.send(Event::MemoryResolved(memory.clone())).await?;
    }
    Ok(memory)
}
