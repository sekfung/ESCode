//! `escode-task.getModelTrajectory`（docs/specs/rust-u2-tasks-index.md §7）。

use std::path::{Path, PathBuf};

use escode_host_services::model_trajectory;

use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

pub(crate) fn trajectory_methods(
    home: &Path,
    data_base_dir: Option<&Path>,
) -> Vec<(&'static str, MethodHandler)> {
    let dirs: Vec<PathBuf> = model_trajectory::model_io_dirs(home, data_base_dir.unwrap_or(home));
    vec![(
        "getModelTrajectory",
        handler(move |arg: Wire| {
            let dirs = dirs.clone();
            Box::pin(async move {
                let params = arg
                    .into_args()
                    .into_iter()
                    .next()
                    .and_then(Wire::into_object)
                    .unwrap_or(serde_json::Value::Null);
                let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
                let limit = params["limit"].as_f64();
                let compare = |a: &str, b: &str| crate::collation::compare_names(a, b);
                Ok(Wire::Object(
                    model_trajectory::read_model_trajectory(&dirs, &task_id, limit, &compare).await,
                ))
            })
        }),
    )]
}
