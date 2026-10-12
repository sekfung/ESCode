/**
 * 同一 IR 的写入串行队列（docs/circuit/README.md §3.3）：
 * 按 location key 排队，前一个任务失败不影响后续任务；事件因此按 revision 单调递增。
 */
export class CircuitWriteQueue {
  private readonly queues = new Map<string, Promise<unknown>>();

  run<T>(key: string, task: () => Promise<T>): Promise<T> {
    const previous = this.queues.get(key) ?? Promise.resolve();
    const next = previous.then(task, task);
    this.queues.set(
      key,
      next.then(
        () => undefined,
        () => undefined,
      ),
    );
    return next;
  }
}
