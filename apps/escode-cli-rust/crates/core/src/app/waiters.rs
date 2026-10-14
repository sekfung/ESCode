//! 挂起确认的单注册表（rust-unified-host.md P1 #8；语义对标 ZCode-rs
//! `app/waiters.rs` 的 `release(owner)`）。
//!
//! 权限确认、ExitPlanMode 审批与 AskUserQuestion 三张表原先各自 `retain`：
//! 会话关闭或 runtime 收口时每处都要记着清三张表，漏一处就是泄漏的挂起应答。
//! 合并成一个注册表后由 [`Waiters::release`] / [`Waiters::clear`] 单点负责，
//! 类型化访问保留（三张表各自泛型不同，互不混淆）。
use std::collections::BTreeMap;

use super::permission_flow::WaitingPermission;
use super::plan_mode::WaitingPlan;
use super::questions::WaitingQuestion;

#[derive(Default)]
pub(super) struct Waiters {
    pub(super) permissions: BTreeMap<String, WaitingPermission>,
    pub(super) plan_exits: BTreeMap<String, WaitingPlan>,
    pub(super) questions: BTreeMap<String, WaitingQuestion>,
}

impl Waiters {
    /// 释放某个 owner（会话）名下的全部挂起确认。等值断言用会话 id，
    /// 与三张表的登记键（callId / questionId）无关。
    pub(super) fn release(&mut self, session: &str) {
        self.permissions
            .retain(|_, waiter| waiter.session != session);
        self.plan_exits
            .retain(|_, waiter| waiter.session != session);
        self.questions.retain(|_, waiter| waiter.session != session);
    }

    /// 运行时收口：全部挂起确认作废（丢弃应答端，等待方按取消语义收尾）。
    pub(super) fn clear(&mut self) {
        self.permissions.clear();
        self.plan_exits.clear();
        self.questions.clear();
    }

    /// 该会话是否仍有挂起确认（测试辅助；生产路径的判定用 `active` 与后台任务表）。
    #[cfg(test)]
    pub(super) fn pending_for(&self, session: &str) -> bool {
        self.permissions.values().any(|w| w.session == session)
            || self.plan_exits.values().any(|w| w.session == session)
            || self.questions.values().any(|w| w.session == session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ToolOutput;
    use crate::domain::permission::Rule;
    use tokio::sync::oneshot;

    fn permission(session: &str) -> (String, WaitingPermission) {
        let (reply, _rx) = oneshot::channel();
        (
            format!("call-{session}"),
            WaitingPermission {
                session: session.into(),
                run: "run".into(),
                call_id: format!("call-{session}"),
                suggested: Vec::<Rule>::new(),
                reply,
            },
        )
    }

    fn plan(session: &str) -> (String, WaitingPlan) {
        let (reply, _rx) = oneshot::channel::<ToolOutput>();
        (
            format!("plan-{session}"),
            WaitingPlan {
                session: session.into(),
                run: "run".into(),
                call_id: format!("plan-{session}"),
                plan: "plan".into(),
                reply,
            },
        )
    }

    fn question(session: &str) -> (String, WaitingQuestion) {
        let (reply, _rx) = oneshot::channel();
        (
            format!("question-{session}"),
            WaitingQuestion {
                session: session.into(),
                run: "run".into(),
                eligible: true,
                reply,
            },
        )
    }

    #[test]
    fn release_drops_every_kind_for_one_owner_only() {
        let mut waiters = Waiters::default();
        for (key, waiter) in [permission("a"), permission("b")] {
            waiters.permissions.insert(key, waiter);
        }
        waiters.plan_exits.insert(plan("a").0, plan("a").1);
        waiters.questions.insert(question("a").0, question("a").1);
        assert!(waiters.pending_for("a") && waiters.pending_for("b"));

        waiters.release("a");

        assert!(!waiters.pending_for("a"), "owner 的全部挂起确认已释放");
        assert!(waiters.pending_for("b"), "其他 owner 的挂起确认不受影响");
        assert_eq!(waiters.permissions.len(), 1);
    }

    #[test]
    fn clear_drops_everything() {
        let mut waiters = Waiters::default();
        waiters
            .permissions
            .insert(permission("a").0, permission("a").1);
        waiters.plan_exits.insert(plan("a").0, plan("a").1);
        waiters.questions.insert(question("a").0, question("a").1);
        waiters.clear();
        assert!(!waiters.pending_for("a"));
    }
}
