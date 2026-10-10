import type { ISerialService } from "@zcode/services";

/**
 * 多个串口面板可能同时可见，而 Host 的 setWatching 是单个布尔：按服务实例做引用计数，
 * 第一个可见面板开启轮询、最后一个隐藏的面板关闭轮询，避免一个面板隐藏时停掉其他面板的热插拔刷新。
 */
const watchCounts = new WeakMap<ISerialService, number>();

export function acquireSerialWatch(service: ISerialService): () => void {
  const count = watchCounts.get(service) ?? 0;
  watchCounts.set(service, count + 1);
  if (count === 0) void service.setWatching({ watching: true }).catch(() => {});
  let released = false;
  return () => {
    if (released) return;
    released = true;
    const remaining = (watchCounts.get(service) ?? 1) - 1;
    watchCounts.set(service, remaining);
    if (remaining === 0) void service.setWatching({ watching: false }).catch(() => {});
  };
}
