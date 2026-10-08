/**
 * 工作流宿主（Rust runtime 用）的「停止后等结算」：stop 请求在 run 的结算通知（`runSettled`）发出之后才应答。
 *
 * 修复原因：Node 里终态通知由 run 的 waiter 在 cancel 之后异步铸造，与 StopWorkflowRun 工具结果之间本就是竞态，
 * 同进程时通知几乎总能赶上下一个步边界。Rust 经宿主子进程多一跳转发，慢机器上工具结果先到、通知落到下一次
 * 模型请求（CI 复现：zcode-cli-rust-workflow-resume）。宿主输出按行有序，先发通知再应答即可让 Rust 确定地
 * 在同一个步边界并入通知。等待以结算事件为准；上限只防 run 永不结算时卡住停止（届时照常应答）。
 */
export const STOP_SETTLE_WAIT_LIMIT_MS = 5000;

export function createSettleWaiter() {
  const settled = new Set<string>();
  const waiters = new Map<string, Array<() => void>>();
  return {
    /** 某 run 的结算通知已发出。 */
    settled(taskId: string): void {
      settled.add(taskId);
      for (const wake of waiters.get(taskId) ?? []) wake();
      waiters.delete(taskId);
    },
    /** 等某 run 的结算通知发出（已发出则立即返回）。 */
    async wait(taskId: string, limitMs = STOP_SETTLE_WAIT_LIMIT_MS): Promise<void> {
      if (settled.has(taskId)) return;
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, limitMs);
        const list = waiters.get(taskId) ?? [];
        list.push(() => {
          clearTimeout(timer);
          resolve();
        });
        waiters.set(taskId, list);
      });
    },
  };
}
