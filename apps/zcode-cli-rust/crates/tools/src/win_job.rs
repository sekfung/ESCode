//! Windows Job Object：Bash 进程树的回收边界，对应 TS `adapters/src/mcp/windows-job-object.ts`。
//! Git Bash 的 MSYS 子进程会丢失 Windows 父子链，`taskkill /T` 在 leader 已退出后找不到后代，
//! 后代继续持有输出管道。放入带 KILL_ON_JOB_CLOSE 的 Job 后，终止或丢弃 Job 即回收整棵树。
use std::{mem::size_of, ptr::null};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
    },
};

pub(super) struct Job(HANDLE);

// HANDLE 是内核对象句柄，可跨线程使用；Job 只在拥有它的 run() 内被访问。
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    /// 创建 Job 并把 `pid` 放入；失败时返回 None，由调用方退回 taskkill（与 TS 附加失败时的降级一致）。
    pub(super) fn attach(pid: u32) -> Option<Self> {
        unsafe {
            let job = CreateJobObjectW(null(), null());
            if job.is_null() {
                return None;
            }
            let job = Self(job);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return None;
            }
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return None;
            }
            let assigned = AssignProcessToJobObject(job.0, process);
            CloseHandle(process);
            (assigned != 0).then_some(job)
        }
    }

    pub(super) fn terminate(&self) -> bool {
        unsafe { TerminateJobObject(self.0, 1) != 0 }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // KILL_ON_JOB_CLOSE：关闭最后一个句柄即回收仍存活的后代，异常路径也不会泄漏进程。
        unsafe {
            CloseHandle(self.0);
        }
    }
}
