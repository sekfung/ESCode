/** 资源管理器的 Chromium 进程角色 pid 快照。 */
export interface ChromiumProcessRolePids {
  mainPid: number;
  mainWindowRendererPids: ReadonlySet<number>;
  guestRendererPids: ReadonlySet<number>;
  hostPids: ReadonlySet<number>;
  schedulerPids: ReadonlySet<number>;
}
