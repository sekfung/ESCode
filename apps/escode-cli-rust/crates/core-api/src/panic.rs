//! 任务 panic 捕获（docs/specs/rust-unified-host.md「实现质量对齐工作包」P0#1）：
//! 让「无论成功、失败还是 panic 都必须发出终态」的任务在 panic 后仍返回普通错误值，
//! 不把 panic 传播给调用方、不留下永远等不到结果的等待方（run panic 曾让 Engine 关闭路径挂死）。
use futures_util::FutureExt;
use std::{any::Any, future::Future, panic::AssertUnwindSafe};

/// 运行 `future`；panic 时返回 [`Panicked`]，不再向外传播。
///
/// 只用于「无论成功、失败还是 panic 都必须发出终态」的任务。panic 之后 `future` 持有的状态
/// 可能处于中间状态，调用方只能把它当失败处理，不能继续使用其中的数据。
pub async fn catch_panic<F: Future>(future: F) -> Result<F::Output, Panicked> {
    AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(Panicked::from_payload)
}

/// 被捕获的 panic；`message` 是 panic 时给出的字符串（非字符串载荷时为固定说明）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panicked {
    pub message: String,
}

impl Panicked {
    fn from_payload(payload: Box<dyn Any + Send>) -> Self {
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_owned());
        Self { message }
    }
}
