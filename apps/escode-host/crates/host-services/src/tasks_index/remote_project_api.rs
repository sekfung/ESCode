//! `TasksIndex` 的 remote-projects 方法（实现见 `remote_project.rs`）。

use super::{TasksIndex, TasksIndexError, remote_project};
use serde_json::Value;

impl TasksIndex {
    pub fn remote_project_read_directory(&self) -> Result<Value, TasksIndexError> {
        remote_project::read_directory(&self.connection)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn remote_project_save_host(&self, input: &Value) -> Result<Value, TasksIndexError> {
        remote_project::save_host(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn remote_project_bind_host(&self, input: &Value) -> Result<Value, TasksIndexError> {
        remote_project::bind_host(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn remote_project_remove_host(&self, id: &str) -> Result<bool, TasksIndexError> {
        remote_project::remove_host(&self.connection, id)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn remote_project_add_entry(&self, input: &Value) -> Result<Value, TasksIndexError> {
        remote_project::add_entry(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn remote_project_refresh_entry(&self, input: &Value) -> Result<bool, TasksIndexError> {
        remote_project::refresh_entry(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn remote_project_remove_entry(&self, id: &str) -> Result<bool, TasksIndexError> {
        remote_project::remove_entry(&self.connection, id)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }
}
