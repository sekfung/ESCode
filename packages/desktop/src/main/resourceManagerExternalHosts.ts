//! 非 Electron utility process 的 Host（Rust Host）在资源管理器中的注册、采样与捕获开关。
//! 从 `resourceManagerWindow.ts` 拆出以守住 max-lines；对外仍是同一份快照。

import { HostMessageTypes, type HostResourceUsageProcess } from "@escode/shared";
import { requestExternalHostResourceUsage } from "./resourceManagerHostSampling.js";
import { registerExternalNetworkCaptureHost } from "./resourceManagerNetwork.js";

export interface ExternalHostProcess {
  label: string;
  pid: number;
  requestResourceUsage(requestId: string): void;
  cancelResourceUsage(requestId: string): void;
  setNetworkCapture(captureId: string | null): void;
}

const externalHostProcesses = new Map<string, ExternalHostProcess>();
const externalNetworkDisposers = new Map<string, () => void>();

export function registerExternalHostProcess(host: ExternalHostProcess): void {
  externalHostProcesses.set(host.label, host);
  externalNetworkDisposers.get(host.label)?.();
  externalNetworkDisposers.set(
    host.label,
    registerExternalNetworkCaptureHost((captureId) => host.setNetworkCapture(captureId)),
  );
}

export function unregisterExternalHostProcess(label: string): void {
  externalHostProcesses.delete(label);
  externalNetworkDisposers.get(label)?.();
  externalNetworkDisposers.delete(label);
}

export function externalHostProcessesSnapshot(): ExternalHostProcess[] {
  return [...externalHostProcesses.values()];
}

export function externalHostProcess(label: string): ExternalHostProcess | undefined {
  return externalHostProcesses.get(label);
}

export function externalHostRows(): Array<{ label: string; pid: number }> {
  return [...externalHostProcesses.values()].map(({ label, pid }) => ({ label, pid }));
}

export async function collectExternalHostProcesses(
  signal?: AbortSignal,
): Promise<HostResourceUsageProcess[]> {
  const results = await Promise.all(
    [...externalHostProcesses.values()].map((host) =>
      requestExternalHostResourceUsage(
        host.label,
        (message) => {
          if (message.type === HostMessageTypes.ResourceUsageSnapshotRequest) {
            host.requestResourceUsage(message.requestId);
          } else {
            host.cancelResourceUsage(message.requestId);
          }
        },
        undefined,
        signal,
      ),
    ),
  );
  return results.flat();
}
