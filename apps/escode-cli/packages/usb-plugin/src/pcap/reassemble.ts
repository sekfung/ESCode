/**
 * 提交/完成配对：把链路层事件合成逻辑报文（docs/specs/usb-agent-tools.md §7）。
 *
 * 规则来自 usbmon 与 USBPcap 的实际行为：
 * - 控制传输：提交事件带 SETUP，数据阶段与状态在完成事件里；控制 OUT 的数据可能在提交侧。
 * - 批量/中断 OUT：数据在**提交**事件（主机发送时抓）；IN 的数据在**完成**事件。
 *   因此载荷取「完成侧非空则用完成侧，否则用提交侧」，而不是固定取一侧。
 * - 等时：不做数据重组，一次完成事件算一条报文。
 */

import type { UsbExchange, UsbTransfer } from "../types.js";

export interface ReassemblyResult {
  exchanges: UsbExchange[];
  warnings: string[];
}

function deviceKey(transfer: { bus: number; device: number }): string {
  return `${transfer.bus}-${transfer.device}`;
}

export function toExchanges(transfers: UsbTransfer[]): ReassemblyResult {
  const warnings: string[] = [];
  const sorted = [...transfers].sort((left, right) => left.ts - right.ts);
  /** 每个端点一条待配对的提交队列；同一端点上的提交按顺序与完成配对（URB 不保证按 id 命中）。 */
  const pending = new Map<string, UsbTransfer[]>();
  /** 带 URB id 的提交按 id 直接定位：同一端点上有多个在途 URB 时，完成顺序可能与提交顺序不同。 */
  const byUrbId = new Map<string, UsbTransfer>();
  const exchanges: UsbExchange[] = [];
  let unmatchedCompletions = 0;

  const queueKey = (transfer: UsbTransfer): string =>
    `${deviceKey(transfer)}:${transfer.endpoint}:${transfer.type}`;
  const urbKey = (transfer: UsbTransfer): string | undefined =>
    transfer.urbId ? `${queueKey(transfer)}:${transfer.urbId}` : undefined;

  for (const transfer of sorted) {
    const key = queueKey(transfer);
    if (!transfer.complete) {
      const queue = pending.get(key);
      if (queue) queue.push(transfer);
      else pending.set(key, [transfer]);
      const id = urbKey(transfer);
      if (id) byUrbId.set(id, transfer);
      continue;
    }
    const id = urbKey(transfer);
    const byId = id ? byUrbId.get(id) : undefined;
    const queue = pending.get(key);
    let submit: UsbTransfer | undefined;
    if (transfer.type !== "isochronous") {
      if (byId) {
        submit = byId;
        // 从 FIFO 队列里摘掉同一条，避免它被后续完成事件重复配对。
        const position = queue?.indexOf(byId) ?? -1;
        if (position >= 0) queue?.splice(position, 1);
      } else {
        submit = queue?.shift();
      }
      if (id) byUrbId.delete(id);
      if (!submit) unmatchedCompletions += 1;
    }
    const payload = pickPayload(submit, transfer);
    exchanges.push({
      index: 0, // 统一在排序后赋 1 起的编号
      ts: submit?.ts ?? transfer.ts,
      bus: transfer.bus,
      device: transfer.device,
      busDev: deviceKey(transfer),
      endpoint: submit?.endpoint ?? transfer.endpoint,
      direction: submit?.direction ?? transfer.direction,
      type: submit?.type ?? transfer.type,
      ...(submit?.setup ? { setup: submit.setup } : {}),
      data: payload.data,
      status: transfer.status,
      ...(submit ? { durationUs: transfer.ts - submit.ts } : {}),
      ...(submit?.declaredLength !== undefined ? { expectedLength: submit.declaredLength } : {}),
      ...(transfer.declaredLength !== undefined ? { actualLength: transfer.declaredLength } : {}),
      ...(submit?.dataZeroFilled || transfer.dataZeroFilled ? { dataZeroFilled: true } : {}),
    });
  }

  const leftoverSubmits = [...pending.values()].reduce((total, queue) => total + queue.length, 0);
  if (unmatchedCompletions > 0) {
    warnings.push(`${unmatchedCompletions} 条完成事件没有对应的提交事件（抓包从中途开始或提交事件被丢弃）`);
  }
  if (leftoverSubmits > 0) {
    warnings.push(`${leftoverSubmits} 条提交事件没有等到完成事件（抓包在事务完成前结束）`);
  }

  exchanges.sort((left, right) => left.ts - right.ts);
  exchanges.forEach((exchange, position) => {
    exchange.index = position + 1;
  });
  return { exchanges, warnings };
}

function pickPayload(
  submit: UsbTransfer | undefined,
  complete: UsbTransfer,
): { data: Uint8Array } {
  if (complete.data.length > 0) return { data: complete.data };
  if (submit && submit.data.length > 0) return { data: submit.data };
  return { data: complete.data };
}
