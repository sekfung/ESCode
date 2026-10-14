//! Owned POSIX process-tree snapshot. No global process registry or background polling.
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    time::Instant,
};

#[derive(Clone)]
struct Process {
    parent: i32,
    group: i32,
    started: String,
    zombie: bool,
}
type Table = BTreeMap<i32, Process>;

fn parse(text: &str) -> Table {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let group = fields.next()?.parse().ok()?;
            let zombie = fields.next()?.starts_with('Z');
            let started = fields.collect::<Vec<_>>().join(" ");
            (!started.is_empty()).then_some((
                pid,
                Process {
                    parent,
                    group,
                    started,
                    zombie,
                },
            ))
        })
        .collect()
}
async fn snapshot() -> Result<Table> {
    // 只在终止路径探测；限制大小和查表时间，ps 自身由 kill_on_drop 回收。
    tokio::time::timeout(Duration::from_millis(500), async {
        let mut child = Command::new("/bin/ps")
            .args(["-A", "-o", "pid=,ppid=,pgid=,stat=,lstart="])
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut bytes = vec![];
        child
            .stdout
            .take()
            .unwrap()
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(
            bytes.len() <= 2 * 1024 * 1024,
            "Process table exceeds budget"
        );
        ensure!(child.wait().await?.success(), "Process lookup failed");
        Ok(parse(std::str::from_utf8(&bytes)?))
    })
    .await
    .context("Process lookup timed out")?
}

struct Tree {
    processes: BTreeMap<i32, String>,
    groups: BTreeMap<i32, Option<String>>,
}
impl Tree {
    fn new(root: i32, table: &Table) -> Self {
        let mut tree = Self {
            processes: BTreeMap::new(),
            groups: BTreeMap::new(),
        };
        tree.groups
            .insert(root, table.get(&root).map(|p| p.started.clone()));
        if let Some(p) = table.get(&root) {
            tree.processes.insert(root, p.started.clone());
        }
        tree.refresh(table);
        tree
    }
    fn owns_group(&self, group: i32, table: &Table) -> bool {
        self.groups.get(&group).is_some_and(|identity| {
            // 新进程占用了已退出组长的 PID 时不能对其组发信号。无组长的旧组仍需清理。
            table
                .get(&group)
                .is_none_or(|p| identity.as_ref() == Some(&p.started))
        })
    }
    fn refresh(&mut self, table: &Table) {
        let mut children: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        let mut members: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        for (&pid, process) in table {
            children.entry(process.parent).or_default().push(pid);
            members.entry(process.group).or_default().push(pid);
        }
        let mut queue = std::collections::VecDeque::new();
        for (&pid, identity) in &self.processes {
            if table.get(&pid).is_some_and(|p| &p.started == identity) {
                queue.push_back(pid);
            }
        }
        for &group in self.groups.keys() {
            if self.owns_group(group, table)
                && let Some(pids) = members.get(&group)
            {
                queue.extend(pids);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        while let Some(pid) = queue.pop_front() {
            if pid <= 1 || !seen.insert(pid) {
                continue;
            }
            let process = &table[&pid];
            self.processes.insert(pid, process.started.clone());
            if let Some(pids) = children.get(&pid) {
                queue.extend(pids);
            }
            // 只有组长确认为本次后代，才能接管 job control / PTY 创建的整个组。
            if process.group == pid {
                self.groups.insert(pid, Some(process.started.clone()));
                if let Some(pids) = members.get(&pid) {
                    queue.extend(pids);
                }
            }
        }
    }
    fn live(&self, table: &Table) -> bool {
        self.processes.iter().any(|(pid, identity)| {
            table
                .get(pid)
                .is_some_and(|p| &p.started == identity && !p.zombie)
        })
    }
    fn maybe_live(&self) -> bool {
        self.groups.keys().any(|group| alive(-*group))
            || self.processes.keys().any(|pid| alive(*pid))
    }
    async fn signal(&self, signal: i32, table: &Table) -> Result<()> {
        for &group in self.groups.keys() {
            if self.owns_group(group, table)
                && table.values().any(|p| p.group == group && !p.zombie)
                && let Err(error) = send(-group, signal)
            {
                // MCP 断开 stdin 后可在 ps 与 killpg 之间退出；macOS 此时也可能返回 EPERM。
                // 仅当新快照证明原拥有者已退出才视为成功，不能吞掉仍存活进程的权限错误。
                let fresh = snapshot().await?;
                if self.owns_group(group, &fresh)
                    && fresh.values().any(|p| p.group == group && !p.zombie)
                {
                    return Err(error);
                }
            }
        }
        for (&pid, identity) in &self.processes {
            if table
                .get(&pid)
                .is_some_and(|p| &p.started == identity && !p.zombie)
                && let Err(error) = send(pid, signal)
            {
                let fresh = snapshot().await?;
                if fresh
                    .get(&pid)
                    .is_some_and(|p| &p.started == identity && !p.zombie)
                {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}
/// 任务中止（future 被 drop）时对内层子进程整组补 SIGKILL：`kill_on_drop` 只结束组长，组内后代
/// （Bash 的子 shell、`&` 后台项）会留下（P0#3，对标仓库 `Tracked::drop` 的同一安全论证）。
/// 根进程尚未回收时它的 pid 与进程组 id 都不会被复用，所以向进程组发 KILL 只会命中这棵树；
/// 已回收（`id()` 为 `None`）则不再发信号，避免误杀复用了 pid 的无关进程组。
pub(super) struct GroupKillOnDrop(pub(super) tokio::process::Child);
impl std::ops::Deref for GroupKillOnDrop {
    type Target = tokio::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for GroupKillOnDrop {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for GroupKillOnDrop {
    fn drop(&mut self) {
        let Some(pid) = self.0.id() else {
            return;
        };
        let Ok(group) = libc::pid_t::try_from(pid) else {
            return;
        };
        // SAFETY: killpg 只投递信号；根进程是本进程尚未回收的子进程，进程组 id 不会被复用。
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
}

fn alive(pid: i32) -> bool {
    (unsafe { libc::kill(pid, 0) == 0 })
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
fn send(pid: i32, signal: i32) -> Result<()> {
    if unsafe { libc::kill(pid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error).context("Cannot signal owned process")
}

/// 没有进程表时的退路（对标仓库 posix.rs 的 `fall_back`）：只对根进程组发信号，再回收根进程。
///
/// 安全性：根进程是本进程尚未回收的子进程，它的 pid 与进程组 id 都不会被复用；组内仍有成员时
/// 组 id 也不会分配给新进程，组已不存在时 killpg 返回 ESRCH。已离开根进程组、自建会话的后代
/// 在这条路径上无法追踪——这是没有进程表时的已知限制（其余后代在正常路径由进程表覆盖）。
async fn fall_back(
    child: &mut Child,
    root: i32,
    graceful: bool,
    cause: &anyhow::Error,
) -> Result<()> {
    if graceful {
        let _ = send(-root, libc::SIGTERM);
        let began = Instant::now();
        while began.elapsed() < Duration::from_millis(1500) {
            child.try_wait()?;
            if !alive(-root) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let _ = send(-root, libc::SIGKILL);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        child.try_wait()?;
        if !alive(-root) {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "Owned Bash processes did not terminate (process table unavailable: {cause:#})"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if child.id().is_some() {
        child.wait().await?;
    }
    Ok(())
}

pub(super) async fn terminate(child: &mut Child, pid: u32, graceful: bool) -> Result<()> {
    let root = pid as i32;
    if !graceful && !alive(-root) {
        return Ok(());
    }
    let began = Instant::now();
    let table = match snapshot().await {
        Ok(table) => table,
        // 修复：快照失败（ps 不可用/超时）原先直接报错、一个信号都不发，子树会残留（P0#3）；
        // 无进程表时退到「只对根进程组收口」。
        Err(cause) => return fall_back(child, root, graceful, &cause).await,
    };
    let mut tree = Tree::new(root, &table);
    if graceful {
        tree.signal(libc::SIGTERM, &table).await?;
        while began.elapsed() < Duration::from_millis(1500) {
            child.try_wait()?;
            if !tree.maybe_live() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    // 单次 killpg 与 fork 竞争会漏掉持有管道的后代。保留组所有权，直到当前快照
    // 确认没有活工作进程；SIGKILL 后的组长退出不是整个工具完成的替代事实。
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let table = match snapshot().await {
            Ok(table) => table,
            Err(cause) => return fall_back(child, root, false, &cause).await,
        };
        tree.refresh(&table);
        if !tree.live(&table) {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "Owned Bash processes did not terminate"
        );
        tree.signal(libc::SIGKILL, &table).await?;
        child.try_wait()?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if child.id().is_some() {
        child.wait().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_keep_owned_groups_and_reject_reused_identities() {
        let first = parse(
            "10 1 10 S Mon Sep 21 00:00:00 2026\n11 10 11 S Mon Sep 21 00:00:00 2026\n12 11 11 S Mon Sep 21 00:00:00 2026\n99 1 99 S Mon Sep 21 00:00:00 2026",
        );
        let mut tree = Tree::new(10, &first);
        assert_eq!(tree.processes.len(), 3);
        assert!(!tree.groups.contains_key(&99));
        let next = parse(
            "11 1 11 S Mon Sep 21 00:00:00 2026\n13 1 10 S Mon Sep 21 00:00:01 2026\n14 11 11 S Mon Sep 21 00:00:01 2026\n12 1 12 S Mon Sep 21 00:01:00 2026",
        );
        tree.refresh(&next);
        assert!(tree.processes.contains_key(&13));
        assert!(tree.processes.contains_key(&14));
        assert_ne!(tree.processes[&12], next[&12].started);
        assert!(!tree.groups.contains_key(&12));
        assert!(!tree.owns_group(11, &parse("11 1 11 S Mon Sep 21 00:02:00 2026")));
        assert!(tree.owns_group(10, &next));
    }

    /// P0#3：没有进程表（快照失败）时也必须对根进程组收口，不能一个信号都不发。
    #[tokio::test]
    async fn fall_back_signals_the_root_group_without_a_process_table() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        command.process_group(0);
        let mut child = command.spawn().unwrap();
        let pid = child.id().unwrap() as i32;
        assert!(alive(-pid));
        let cause = anyhow::anyhow!("injected snapshot failure");
        fall_back(&mut child, pid, true, &cause).await.unwrap();
        assert!(!alive(-pid));
        assert!(child.try_wait().unwrap().is_some());
    }

    /// P0#3 对照用例：会话隔离（setsid）与 Drop 整组回收。
    /// 修复前 `process_group(0)` 不脱离控制终端；任务中止时 `kill_on_drop` 只结束组长，后代留在组里。
    #[tokio::test]
    async fn isolate_starts_a_new_session_and_drop_kills_the_whole_group() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        escode_cli_host::process_group::isolate(&mut command);
        let child = GroupKillOnDrop(command.spawn().unwrap());
        let pid = child.id().unwrap() as i32;
        // 新会话首进程的 sid 与 pgid 都等于自身 pid（`/dev/tty` 凭据提示由此立即失败而非挂起）。
        assert_eq!(unsafe { libc::getsid(pid) }, pid);
        assert_eq!(unsafe { libc::getpgid(pid) }, pid);
        assert!(alive(-pid));
        // 不带 wait 直接丢弃（模拟任务被中止）：整组（sh 与其 sleep 后代）都必须被回收。
        drop(child);
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(-pid) {
            assert!(Instant::now() < deadline, "process group survived the drop");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
