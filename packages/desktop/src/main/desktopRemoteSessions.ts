/* eslint-disable max-lines -- Main 的完整薄转发契约集中维护请求与端口关联。 */
import { randomUUID } from "node:crypto";
import { BrowserWindow, MessageChannelMain } from "electron";
import type { MessagePortMain, UtilityProcess as ElectronUtilityProcess } from "electron";
import {
  type BindRemoteWorkspaceSessionContextResult,
  buildRemoteWorkspaceIdentity,
  buildServerRemoteWorkspaceIdentity,
  buildRemoteEnvironmentKey,
  buildSshRemoteHostKey,
  HostMessageTypes,
  HostResponseTypes,
  hostResponseMessageSchema,
  InternalChannels,
  PlatformChannels,
  type RemoteTarget,
  type ProviderProvisioningTrigger,
  type ScopedServicePortPayload,
  type WorkspacePurpose,
  type WindowHostRemoteWorkspaceDescriptor,
  stripRemoteTargetSecrets,
} from "@escode/shared";
import type { RemoteAssetDirs } from "./desktopRuntimeEnv.js";
import type { RustHostRemoteScope } from "./rustHostBridge.js";
import { ProviderProvisioningEnvironmentCoordinator } from "./providerProvisioningEnvironmentCoordinator.js";

interface RemoteWorkspaceSessionContext {
  workspacePath: string;
  workspaceIdentity?: string;
  workspacePurpose?: WorkspacePurpose;
}

type ProfileRemoteTarget = Exclude<RemoteTarget, { kind: "server" }>;

interface ResolvedProfileScope {
  profileId: string;
  sourceServerId: string;
}

interface ResolvedRemoteTarget {
  target: RemoteTarget;
  profileScope?: ResolvedProfileScope;
}

/** 首连 single-flight 键：与 Rust 的 Profile 身份键一致，不含密码/口令/展示字段。 */
function profileTargetIdentityKey(target: ProfileRemoteTarget): string {
  switch (target.kind) {
    case "ssh":
      return JSON.stringify({
        kind: "ssh",
        host: target.host.trim().toLowerCase(),
        port: target.port ?? 22,
        username: target.username.trim(),
        privateKeyPath: target.privateKeyPath?.trim() ?? "",
      });
    case "wsl":
      return JSON.stringify({
        kind: "wsl",
        distro: target.distro?.trim() ?? "",
        user: target.user?.trim() ?? "",
      });
    case "docker":
      return JSON.stringify({ kind: "docker", container: target.container.trim() });
  }
}

interface PendingConnect {
  requestId: string;
  webContentsId: number;
  win: BrowserWindow;
  resolve: (sessionId: string) => void;
  reject: (error: Error) => void;
  /** 连接已提交给 Host 后的取消位：迟到的 attachment 立即收口；首连在建时由 resolution caller 取消。 */
  cancelled?: boolean;
}

/** 首连解析的 single-flight：一个目标身份一条 flight，可被多个 caller 共享。 */
interface RemoteProfileResolutionFlight {
  /** Host 侧 caller key（取消时回传，绝不复用 renderer requestId）。 */
  hostRequestId: string;
  controller: AbortController;
  promise: Promise<{ profileId: string; sourceServerId: string; target: Record<string, unknown> }>;
  /** 是否仍在飞行；最后一个 caller 取消时据此决定是否中止 Host 连接。 */
  inFlight: boolean;
  callers: Set<string>;
}

/** 一个等待首连解析的 caller（窗口 + renderer requestId），取消按它精确命中。 */
interface PendingProfileResolution {
  webContentsId: number;
  requestId: string;
  cancel: (reason: string) => void;
}

interface RemoteAttachmentRoute {
  webContentsId: number;
  /**
   * 这条 route 的服务面 owner（S3-c，spec §10.12）：`node-host` 只服务显式 Web 服务端
   * (`kind: "server"`)；`rust-host` = App 级 Rust Host 的 ssh/wsl/docker remote scope。
   * 两者不共用同一条 route。
   */
  owner: "node-host" | "rust-host";
  /** `rust-host` route 的桥接句柄（dispose 即关闭 `/ws` 与端口）。 */
  rustAttachment?: RustScopeAttachment;
  // Main 只保留端口转发所需的 request/session 关联和脱敏 descriptor；连接/任务事实归 Host。
  descriptor: WindowHostRemoteWorkspaceDescriptor;
  rendererAttachmentId?: string;
  pendingRendererAttachment?: {
    attachmentId: string;
    previousAttachmentId?: string;
    reason: string;
    /** `rust-host` route：候选 attachment 的桥接句柄（ready 后提升为 `rustAttachment`）。 */
    rustAttachment?: RustScopeAttachment;
    /** rust route 在附件（capability/安装）就绪、端口交付后才开始计时。 */
    timeout?: NodeJS.Timeout;
    resolve: () => void;
    reject: (error: Error) => void;
  };
  attachmentState: "attachable" | "closed";
  connectedAtMonotonicMs: number;
  connectFinalized: boolean;
  providerProvisioningDispose?: () => void;
}

/** Rust Host 远端 scope 的 attachment；`onLost` 在 `/ws` 非主动关闭时触发（spec §10.14）。 */
interface RustScopeAttachment {
  dispose(): void;
  onLost?(listener: (reason: RustScopeAttachmentLostReason) => void): void;
  onClosed?(listener: (event: RustScopeAttachmentClosedEvent) => void): void;
}

interface RustScopeAttachmentClosedEvent {
  code?: number;
  reasonText?: string;
}

type RustScopeAttachmentLostReason =
  | { reason: "host-exited"; exit?: unknown }
  | { reason: "socket-closed"; code?: number; reasonText?: string };

interface PendingProviderProvisioningExecution {
  readonly child: ElectronUtilityProcess;
  readonly trigger: ProviderProvisioningTrigger;
  readonly startedAtMonotonicMs: number;
  readonly resolve: () => void;
  readonly reject: (error: Error) => void;
}

function closeMessagePort(port: MessagePortMain | undefined): void {
  if (!port) return;
  try {
    port.close();
  } catch {
    // MessagePort 关闭失败不影响 Host 内 attachment 的 close/dispose 幂等收口。
  }
}

export function createRemoteWorkspaceSessionManager(options: {
  logger: {
    info: (...args: unknown[]) => void;
    warn: (...args: unknown[]) => void;
    error: (...args: unknown[]) => void;
  };
  windowHostProcessMap: Map<number, ElectronUtilityProcess>;
  /**
   * 仅 Web `kind: "server"` 的远程 workspace 需要 per-window Node Host；ssh/wsl/docker 由 App 级
   * Rust Host 服务（S5-a/S5-b）。首次 Connect/Bind 时懒创建并写回 `windowHostProcessMap`。
   */
  ensureWindowHost?: (win: BrowserWindow) => Promise<ElectronUtilityProcess>;
  resolveRemoteAssetDirs: () => RemoteAssetDirs;
  resolveWslTarget?: (
    target: Extract<RemoteTarget, { kind: "wsl" }>,
  ) => Promise<Extract<RemoteTarget, { kind: "wsl" }>>;
  /**
   * 首连解析（S5-a，spec §12.2）：无 Profile 归属的 ssh/wsl/docker 目标交给 Rust Host
   * 匹配/创建 Profile、连接并回填 `sourceServerId`。缺省 = 当前 Host 不服务远端 workspace。
   */
  resolveRustRemoteProfile?: (
    request: {
      profileId?: string;
      target?: Record<string, unknown>;
      requestId?: string;
    },
    options?: { signal?: AbortSignal },
  ) => Promise<{
    profileId: string;
    sourceServerId: string;
    target: Record<string, unknown>;
  }>;
  /**
   * 首连取消（S5-a cancel，spec §12.2）：Host 按 caller key 丢弃在建 transport，关闭 SSH socket。
   * 缺省 = 该 Host 不支持取消；取消仍会结束本地等待，但不会中止远端连接。
   */
  cancelRustRemoteProfile?: (requestId: string) => Promise<unknown>;
  createMessageChannel?: () => {
    port1: MessagePortMain;
    port2: MessagePortMain;
  };
  /**
   * 远端 scope 的 attachment（S3-c，spec §10.12）：`RemoteTarget` 携带 `profileId` 时，连接事实由
   * Rust Host 按 Connection Profile 建立，Main 只把 `/ws` 桥接到 renderer。缺省 = 该 Host 不服务
   * 远端 scope：携带 `profileId` 的连接显式报错（不回退 per-window Node Host）。
   */
  attachRustRemoteScope?: (scope: RustHostRemoteScope) => Promise<{
    port: unknown;
    dispose(): void;
    onLost?(listener: (reason: RustScopeAttachmentLostReason) => void): void;
    onClosed?(listener: (event: RustScopeAttachmentClosedEvent) => void): void;
  }>;
  rendererAttachmentReadyTimeoutMs?: number;
  monotonicNowMs?: () => number;
  providerProvisioningCoordinator?: ProviderProvisioningEnvironmentCoordinator;
}) {
  const pendingByRequestKey = new Map<string, PendingConnect>();
  const routesBySessionId = new Map<string, RemoteAttachmentRoute>();
  const listenedHosts = new WeakSet<ElectronUtilityProcess>();
  const pendingProviderProvisioningExecutions = new Map<
    string,
    PendingProviderProvisioningExecution
  >();
  const providerProvisioningCoordinator =
    options.providerProvisioningCoordinator ?? new ProviderProvisioningEnvironmentCoordinator();
  let appShutdownStarted = false;
  const monotonicNowMs = options.monotonicNowMs ?? (() => performance.now());
  const pendingProfileResolutions = new Map<string, RemoteProfileResolutionFlight>();
  const pendingProfileResolutionCallers = new Map<string, PendingProfileResolution>();

  /**
   * 非 server 目标一律由 Rust Host 服务（S5-a）。已有完整 Profile 归属时直接路由；
   * 首连或历史快照缺字段时，由 Rust Host 创建/匹配 Profile 并回填 `sourceServerId`。
   */
  async function resolveRemoteTarget(
    target: RemoteTarget,
    caller: { webContentsId: number; requestId: string },
  ): Promise<ResolvedRemoteTarget> {
    if (target.kind === "server") return { target };
    const resolvedTarget =
      target.kind === "wsl" && options.resolveWslTarget
        ? await options.resolveWslTarget(target)
        : target;
    const profileId = resolvedTarget.profileId?.trim();
    const sourceServerId = resolvedTarget.sourceServerId?.trim();
    if (profileId && sourceServerId) {
      return {
        target: stripRemoteTargetSecrets(resolvedTarget),
        profileScope: { profileId, sourceServerId },
      };
    }
    const resolveRemoteProfile = options.resolveRustRemoteProfile;
    if (!resolveRemoteProfile) {
      throw Object.assign(
        new Error("当前 Host 不服务 Connection Profile 远端 workspace（Rust Host 未启用）"),
        { code: "REMOTE_RUST_HOST_UNAVAILABLE" as const },
      );
    }
    const key = profileId
      ? `profile\0${profileId}`
      : `target\0${profileTargetIdentityKey(resolvedTarget)}`;
    const callerKey = requestKey(caller.webContentsId, caller.requestId);
    if (pendingProfileResolutionCallers.has(callerKey)) {
      throw new Error(`远程连接 requestId 重复，requestId=${caller.requestId}`);
    }
    let flight = pendingProfileResolutions.get(key);
    if (!flight) {
      flight = startProfileResolutionFlight(key, resolvedTarget, profileId, resolveRemoteProfile);
    }
    const activeFlight = flight;
    let callerSettled = false;
    let cancelCaller!: (reason: string) => void;
    // 每个 caller 独立等待：取消一个 caller 不影响共享同一 flight 的其它窗口（spec §12.2）。
    const cancelled = new Promise<never>((_resolve, reject) => {
      cancelCaller = (reason: string) => {
        if (callerSettled) return;
        reject(
          Object.assign(new Error(reason), {
            code: "REMOTE_CONNECT_CANCELLED" as const,
          }),
        );
      };
    });
    void cancelled.catch(() => undefined);
    pendingProfileResolutionCallers.set(callerKey, {
      webContentsId: caller.webContentsId,
      requestId: caller.requestId,
      cancel: cancelCaller,
    });
    activeFlight.callers.add(callerKey);
    let resolved: { profileId: string; sourceServerId: string; target: Record<string, unknown> };
    try {
      resolved = await Promise.race([activeFlight.promise, cancelled]);
    } finally {
      callerSettled = true;
      pendingProfileResolutionCallers.delete(callerKey);
      activeFlight.callers.delete(callerKey);
      // 最后一个 caller 离开且 flight 仍在飞行：中止 Host 连接，避免卡住的 SSH socket 泄漏。
      if (activeFlight.inFlight && activeFlight.callers.size === 0) {
        cancelProfileResolutionFlight(key, activeFlight, "远程连接已取消");
      }
    }
    if (appShutdownStarted) {
      throw new Error("应用正在退出，无法创建远程工作区连接");
    }
    const profileTarget = resolved.target;
    if (profileTarget.kind !== resolvedTarget.kind) {
      throw new Error(
        `Rust Host 返回的 Profile transport ${String(profileTarget.kind)} 与目标 ${resolvedTarget.kind} 不一致`,
      );
    }
    const effectiveTarget = stripRemoteTargetSecrets({
      ...(profileTarget as unknown as ProfileRemoteTarget),
      profileId: resolved.profileId,
      sourceServerId: resolved.sourceServerId,
    });
    return {
      target: effectiveTarget,
      profileScope: {
        profileId: resolved.profileId,
        sourceServerId: resolved.sourceServerId,
      },
    };
  }

  /** 建立首连解析 flight：一个目标身份一条，Host 请求 key 单独生成（不复用 renderer requestId）。 */
  function startProfileResolutionFlight(
    key: string,
    target: ProfileRemoteTarget,
    profileId: string | undefined,
    resolveRemoteProfile: NonNullable<typeof options.resolveRustRemoteProfile>,
  ): RemoteProfileResolutionFlight {
    const hostRequestId = randomUUID();
    const controller = new AbortController();
    const request = profileId
      ? { profileId, requestId: hostRequestId }
      : { target: { ...target } as Record<string, unknown>, requestId: hostRequestId };
    const flight = {
      hostRequestId,
      controller,
      inFlight: true,
      callers: new Set<string>(),
      promise: undefined as unknown as RemoteProfileResolutionFlight["promise"],
    };
    flight.promise = resolveRemoteProfile(request, { signal: controller.signal }).finally(() => {
      flight.inFlight = false;
      if (pendingProfileResolutions.get(key) === flight) {
        pendingProfileResolutions.delete(key);
      }
    });
    // 最后一个 caller 已取消时没人再消费 reject：这里显式吞掉，避免 unhandled rejection。
    void flight.promise.catch(() => undefined);
    pendingProfileResolutions.set(key, flight);
    return flight;
  }

  /** 最后一个 caller 取消：中止本地等待，并通知 Host 丢弃在建 transport（关闭 SSH socket）。 */
  function cancelProfileResolutionFlight(
    key: string,
    flight: RemoteProfileResolutionFlight,
    reason: string,
  ): void {
    if (!flight.inFlight) return;
    flight.inFlight = false;
    if (pendingProfileResolutions.get(key) === flight) {
      pendingProfileResolutions.delete(key);
    }
    options.logger.info("[connect-remote] canceling in-flight profile resolution", {
      hostRequestId: flight.hostRequestId,
      reason,
    });
    flight.controller.abort();
    void Promise.resolve(options.cancelRustRemoteProfile?.(flight.hostRequestId)).catch(
      (error: unknown) => {
        options.logger.warn("[connect-remote] failed to cancel in-flight profile resolution", {
          message: error instanceof Error ? error.message : String(error),
          reason,
        });
      },
    );
  }

  /** 远端 scope 的授权三元组（spec §10.11/§10.12）；descriptor 未绑定时就是契约错误。 */
  function remoteScopeOf(route: RemoteAttachmentRoute): RustHostRemoteScope {
    const descriptor = route.descriptor;
    const sourceServerId = descriptor.sourceServerId?.trim();
    const profileId = descriptor.target.kind === "server" ? undefined : descriptor.target.profileId;
    if (!sourceServerId || !profileId?.trim() || !descriptor.workspaceIdentity?.trim()) {
      throw Object.assign(
        new Error(
          `远程 route 缺少 scope 字段（sourceServerId/profileId/workspaceIdentity），sessionId=${descriptor.remoteSessionId}`,
        ),
        { code: "REMOTE_SCOPE_INCOMPLETE" as const },
      );
    }
    return {
      sourceServerId,
      profileId: profileId.trim(),
      workspaceIdentity: descriptor.workspaceIdentity,
    };
  }

  function requestKey(webContentsId: number, requestId: string): string {
    return `${webContentsId}\0${requestId}`;
  }

  async function getWindowHost(win: BrowserWindow): Promise<ElectronUtilityProcess> {
    let child = options.windowHostProcessMap.get(win.webContents.id);
    if ((!child || child.pid == null) && options.ensureWindowHost) {
      child = await options.ensureWindowHost(win);
      options.windowHostProcessMap.set(win.webContents.id, child);
    }
    if (!child || child.pid == null) {
      throw new Error(`未找到窗口 Local Host，windowId=${win.webContents.id}`);
    }
    ensureHostListener(child, win.webContents.id);
    return child;
  }

  function getWindowHostSync(win: BrowserWindow): ElectronUtilityProcess {
    const child = options.windowHostProcessMap.get(win.webContents.id);
    if (!child || child.pid == null) {
      throw new Error(`未找到窗口 Local Host，windowId=${win.webContents.id}`);
    }
    ensureHostListener(child, win.webContents.id);
    return child;
  }

  function createMessageChannel() {
    return options.createMessageChannel?.() ?? new MessageChannelMain();
  }

  function emitConnectionLog(
    win: BrowserWindow,
    payload: {
      requestId?: string;
      sessionId?: string;
      level: "info" | "warn" | "error";
      message: string;
    },
  ): void {
    if (win.isDestroyed() || win.webContents.isDestroyed()) return;
    win.webContents.send(PlatformChannels.RemoteConnectionLog, {
      label: `window-host-${win.webContents.id}`,
      requestId: payload.requestId,
      sessionId: payload.sessionId,
      level: payload.level,
      source: "window-host-controller",
      message: payload.message,
      // 改造曾把完整 ISO 时间直接交给连接日志 UI，长时间戳会挤压 flex 日志列并换行。
      // 这里恢复既有的紧凑时钟展示契约，Host 的 requestId 路由和原始日志内容保持不变。
      timestamp: new Date().toLocaleTimeString(undefined, {
        hour12: false,
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
      }),
    });
  }

  function detachServicePort(
    child: ElectronUtilityProcess,
    attachmentId: string,
    reason: string,
  ): void {
    try {
      child.postMessage({
        type: HostMessageTypes.DetachServicePort,
        attachmentId,
      });
    } catch (error) {
      // 候选 port 的回收属于幂等清理，不能因 Host 已退出导致 ready promise 再次悬空。
      options.logger.warn("[window-host-remote] detach renderer attachment failed", {
        attachmentId,
        reason,
        error,
      });
    }
  }

  /**
   * Desktop Main → Preload → Renderer 的 remote scope 元数据（spec §10.12）。
   *
   * `sourceServerId` 是 Host 的权威 Source 身份，`workspacePath` / `workspaceIdentity` 是 bind 后的
   * 权威 scope。Renderer 只消费这些字段，不再按 target 或 identity 字符串反推；首次 connect 与
   * 目录 bind 换代都必须携带，否则设备管理会退化成「未识别设备」兼容分支。
   */
  function buildScopedServicePortPayload(
    descriptor: WindowHostRemoteWorkspaceDescriptor,
    attachmentId: string,
    workspace: { workspacePath: string; workspaceIdentity: string },
  ): ScopedServicePortPayload {
    return {
      attachmentId,
      sessionId: descriptor.remoteSessionId,
      target: descriptor.target,
      ...(descriptor.sourceServerId ? { sourceServerId: descriptor.sourceServerId } : {}),
      workspacePath: workspace.workspacePath,
      workspaceIdentity: workspace.workspaceIdentity,
    };
  }

  /**
   * 把一个 route 的 attachment 交付给 renderer。
   *
   * `node-host` route：先让 per-window Node Host 绑定一个 MessagePort（仅 Web 服务端）。
   * `rust-host` route（S3-c，spec §10.12）：向 Rust Host 申请 scoped capability 并桥接 `/ws`；
   * 失败是显式错误（不换 Node Host 服务同一 workspace），候选 attachment 立即收口。
   */
  async function attachRendererPort(
    win: BrowserWindow,
    route: RemoteAttachmentRoute,
    reason: string,
  ): Promise<void> {
    if (win.isDestroyed() || win.webContents.isDestroyed()) {
      throw new Error("窗口已关闭，无法 attachment 远程 workspace");
    }
    const rustRoute = route.owner === "rust-host";
    const child = rustRoute ? undefined : await getWindowHost(win);
    const descriptor = route.descriptor;
    const workspacePath = descriptor.workspacePath;
    const workspaceIdentity = descriptor.workspaceIdentity;
    if (!workspacePath || !workspaceIdentity) {
      throw new Error(
        `远程 descriptor 缺少 workspace scope，sessionId=${descriptor.remoteSessionId}`,
      );
    }
    // scope 先解析：缺字段属于契约错误，不应先建端口再失败。
    const rustScope = rustRoute ? remoteScopeOf(route) : undefined;
    const attachmentId = randomUUID();
    const scopedServicePort = buildScopedServicePortPayload(descriptor, attachmentId, {
      workspacePath,
      workspaceIdentity,
    });
    const previousAttachmentId = route.rendererAttachmentId;
    const superseded = route.pendingRendererAttachment;
    if (superseded) {
      if (superseded.timeout) clearTimeout(superseded.timeout);
      route.pendingRendererAttachment = undefined;
      if (child) detachServicePort(child, superseded.attachmentId, "superseded");
      superseded.rustAttachment?.dispose();
      superseded.reject(
        new Error(`renderer attachment 已被后续换代替代，sessionId=${descriptor.remoteSessionId}`),
      );
    }

    return new Promise<void>((resolve, reject) => {
      // renderer attachment ready 只覆盖「端口送达 → renderer 注册完服务」这段握手。
      // rust route 的端口要等 Host 建连（首次连接含安装，spec §11.3）后才能交付，
      // 因此计时器在附件就绪后才启动；连接/安装进度由 Source Server 状态机发布。
      const armReadyTimeout = (): NodeJS.Timeout => {
        const timer = setTimeout(() => {
          const pending = route.pendingRendererAttachment;
          if (!pending || pending.attachmentId !== attachmentId) return;
          route.pendingRendererAttachment = undefined;
          if (child) detachServicePort(child, attachmentId, "ready-timeout");
          pending.rustAttachment?.dispose();
          const error = new Error(
            `renderer attachment ready 超时，sessionId=${descriptor.remoteSessionId}`,
          );
          options.logger.warn("[window-host-remote] renderer attachment ready timeout", {
            sessionId: descriptor.remoteSessionId,
            attachmentId,
            reason,
          });
          reject(error);
        }, options.rendererAttachmentReadyTimeoutMs ?? 15_000);
        timer.unref?.();
        return timer;
      };
      route.pendingRendererAttachment = {
        attachmentId,
        previousAttachmentId,
        reason,
        ...(rustRoute ? {} : { timeout: armReadyTimeout() }),
        resolve,
        reject,
      };

      if (rustRoute) {
        const attachRustRemoteScope = options.attachRustRemoteScope;
        if (!attachRustRemoteScope || !rustScope) {
          const error = Object.assign(
            new Error("当前 Host 不服务 Connection Profile 远端 workspace（Rust Host 未启用）"),
            { code: "REMOTE_RUST_HOST_UNAVAILABLE" as const },
          );
          route.pendingRendererAttachment = undefined;
          reject(error);
          return;
        }
        void attachRustRemoteScope(rustScope)
          .then((attachment) => {
            const pending = route.pendingRendererAttachment;
            // ready 超时/被换代/route 已释放：迟到的候选立即收口，不泄漏 `/ws`。
            if (!pending || pending.attachmentId !== attachmentId) {
              attachment.dispose();
              return;
            }
            pending.rustAttachment = attachment;
            attachment.onLost?.((reason) =>
              handleRustAttachmentLost(win, route, attachment, reason),
            );
            attachment.onClosed?.((event) =>
              handleRustAttachmentLost(win, route, attachment, {
                reason: "socket-closed",
                ...event,
              }),
            );
            pending.timeout = armReadyTimeout();
            win.webContents.postMessage(InternalChannels.ScopedServicePort, scopedServicePort, [
              attachment.port as MessagePortMain,
            ]);
          })
          .catch((error: unknown) => {
            const pending = route.pendingRendererAttachment;
            if (!pending || pending.attachmentId !== attachmentId) return;
            route.pendingRendererAttachment = undefined;
            if (pending.timeout) clearTimeout(pending.timeout);
            reject(error instanceof Error ? error : new Error(String(error)));
          });
        return;
      }

      const { port1, port2 } = createMessageChannel();
      try {
        child.postMessage(
          {
            type: HostMessageTypes.AttachServicePort,
            requestId: randomUUID(),
            attachmentId,
            clientMode: "desktop-continuous",
            scope: {
              kind: "remote",
              remoteSessionId: descriptor.remoteSessionId,
              workspacePath: descriptor.workspacePath,
              workspaceIdentity: descriptor.workspaceIdentity,
            },
          },
          [port2],
        );
        win.webContents.postMessage(InternalChannels.ScopedServicePort, scopedServicePort, [port1]);
      } catch (error) {
        const pending = route.pendingRendererAttachment;
        if (pending?.timeout) clearTimeout(pending.timeout);
        route.pendingRendererAttachment = undefined;
        detachServicePort(child, attachmentId, "delivery-failed");
        closeMessagePort(port1);
        closeMessagePort(port2);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  function confirmRendererAttachmentReady(
    webContentsId: number,
    payload: { sessionId: string; attachmentId: string },
  ): void {
    const route = routesBySessionId.get(payload.sessionId);
    const pending = route?.pendingRendererAttachment;
    if (
      !route ||
      route.webContentsId !== webContentsId ||
      !pending ||
      pending.attachmentId !== payload.attachmentId
    ) {
      options.logger.warn("[window-host-remote] ignore stale renderer attachment ready", {
        webContentsId,
        sessionId: payload.sessionId,
        attachmentId: payload.attachmentId,
      });
      return;
    }

    if (pending.timeout) clearTimeout(pending.timeout);
    route.pendingRendererAttachment = undefined;
    const child =
      route.owner === "node-host"
        ? options.windowHostProcessMap.get(route.webContentsId)
        : undefined;
    if (route.owner === "node-host" && (!child || child.pid == null)) {
      pending.reject(new Error(`窗口 Local Host 已退出，sessionId=${payload.sessionId}`));
      return;
    }
    route.rendererAttachmentId = pending.attachmentId;
    if (route.owner === "rust-host") {
      // 候选 attachment 就绪后才淘汰旧句柄：换代期间 renderer 仍可用旧 port。
      route.rustAttachment?.dispose();
      route.rustAttachment = pending.rustAttachment;
    } else if (pending.previousAttachmentId && child) {
      // Host generation bind 会 fail-closed 立即失效 A；Main 仍需等 renderer 注册 B
      // 后再提升 route 并做幂等清理。reload 复挂没有 generation 换代，也因此不会提前拆掉可用旧 port。
      detachServicePort(child, pending.previousAttachmentId, "candidate-promoted");
    }
    options.logger.info(
      `[window-host-remote] renderer attachment ready, sessionId=${payload.sessionId}, reason=${pending.reason}`,
    );
    route.connectFinalized = true;
    pending.resolve();
  }

  function handleConnected(
    child: ElectronUtilityProcess,
    webContentsId: number,
    requestId: string,
    descriptor: WindowHostRemoteWorkspaceDescriptor,
  ): void {
    const key = requestKey(webContentsId, requestId);
    const pending = pendingByRequestKey.get(key);
    if (descriptor.target.kind !== "server") {
      pendingByRequestKey.delete(key);
      child.postMessage({
        type: HostMessageTypes.DisposeRemoteWorkspaceSession,
        requestId: randomUUID(),
        remoteSessionId: descriptor.remoteSessionId,
      });
      pending?.reject(
        Object.assign(new Error(`远程 ${descriptor.target.kind} workspace 必须由 Rust Host 服务`), {
          code: "REMOTE_RUST_HOST_REQUIRED" as const,
        }),
      );
      return;
    }
    if (!pending) {
      child.postMessage({
        type: HostMessageTypes.DisposeRemoteWorkspaceSession,
        requestId: randomUUID(),
        remoteSessionId: descriptor.remoteSessionId,
      });
      return;
    }
    pendingByRequestKey.delete(key);
    const route: RemoteAttachmentRoute = {
      webContentsId,
      owner: "node-host",
      descriptor,
      attachmentState: "attachable",
      connectedAtMonotonicMs: monotonicNowMs(),
      connectFinalized: false,
    };
    routesBySessionId.set(descriptor.remoteSessionId, route);
    const environmentKey = buildRemoteEnvironmentKey(descriptor.target);
    const registration = providerProvisioningCoordinator.register(
      environmentKey,
      descriptor.remoteSessionId,
      (trigger) =>
        executeProviderProvisioning(child, descriptor.remoteSessionId, environmentKey, trigger),
    );
    route.providerProvisioningDispose = registration.dispose;
    void registration.initialSync
      .then(() => attachRendererPort(pending.win, route, "connect"))
      .then(() => {
        emitConnectionLog(pending.win, {
          requestId,
          sessionId: descriptor.remoteSessionId,
          level: "info",
          message: `远程 ${descriptor.target.kind} workspace 已连接`,
        });
        pending.resolve(descriptor.remoteSessionId);
      })
      .catch((error: unknown) => {
        route.providerProvisioningDispose?.();
        routesBySessionId.delete(descriptor.remoteSessionId);
        child.postMessage({
          type: HostMessageTypes.DisposeRemoteWorkspaceSession,
          requestId: randomUUID(),
          remoteSessionId: descriptor.remoteSessionId,
        });
        emitConnectionLog(pending.win, {
          requestId,
          sessionId: descriptor.remoteSessionId,
          level: "error",
          message: error instanceof Error ? error.message : String(error),
        });
        pending.reject(error instanceof Error ? error : new Error(String(error)));
      });
  }

  function executeProviderProvisioning(
    child: ElectronUtilityProcess,
    remoteSessionId: string,
    environmentKey: string,
    trigger: ProviderProvisioningTrigger,
  ): Promise<void> {
    const requestId = randomUUID();
    return new Promise<void>((resolve, reject) => {
      pendingProviderProvisioningExecutions.set(requestId, {
        child,
        trigger,
        startedAtMonotonicMs: monotonicNowMs(),
        resolve,
        reject,
      });
      try {
        child.postMessage({
          type: HostMessageTypes.ProviderProvisioningExecute,
          requestId,
          environmentKey,
          remoteSessionId,
          trigger,
        });
      } catch (error) {
        pendingProviderProvisioningExecutions.delete(requestId);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  function detachRouteAttachments(
    route: RemoteAttachmentRoute,
    reason: string,
    error: Error,
  ): void {
    const child = options.windowHostProcessMap.get(route.webContentsId);
    if (route.pendingRendererAttachment) {
      const pending = route.pendingRendererAttachment;
      if (pending.timeout) clearTimeout(pending.timeout);
      route.pendingRendererAttachment = undefined;
      if (child) detachServicePort(child, pending.attachmentId, reason);
      pending.reject(error);
    }
    if (route.rendererAttachmentId) {
      if (child) detachServicePort(child, route.rendererAttachmentId, reason);
      route.rendererAttachmentId = undefined;
    }
  }

  /**
   * 远端 scope 的桥接丢失（S3-c-3a，spec §10.14）：Host 结束 `/ws` 或 Host 进程退出。
   *
   * 修复：此前 bridge 只记录日志，Rust route 的 `onLost` 也无人订阅。链路 EOF 后窗口继续显示
   * “已连接”，调用全部命中失效句柄。这里按 Node Host 的 `connection-closed` 语义收口 route，
   * 并用既有 `RemoteSessionClosed` 通知 renderer；重连仍由 renderer 的 `connectRemote` 重新签发
   * capability，不在旧端口上原地恢复。
   */
  function handleRustAttachmentLost(
    win: BrowserWindow,
    route: RemoteAttachmentRoute,
    attachment: RustScopeAttachment,
    lost: RustScopeAttachmentLostReason,
  ): void {
    const sessionId = route.descriptor.remoteSessionId;
    if (routesBySessionId.get(sessionId) !== route) return;
    const pending = route.pendingRendererAttachment;
    const active = route.rustAttachment === attachment;
    const candidate = pending?.rustAttachment === attachment;
    // 换代/主动 dispose 后迟到的关闭事件不属于当前 attachment。
    if (!active && !candidate) return;

    const message =
      lost.reason === "host-exited"
        ? "Rust Host 已退出，远程 workspace 连接已断开"
        : "Source Server 连接已断开，请重试";
    const reasonCode =
      lost.reason === "host-exited" ? "remote-host-exited" : "source-server-offline";
    if (!active) {
      // bind 换代的候选失败：只让本次 bind 显式失败，旧 attachment 继续服务 renderer。
      const pending = route.pendingRendererAttachment!;
      if (pending.timeout) clearTimeout(pending.timeout);
      route.pendingRendererAttachment = undefined;
      attachment.dispose();
      pending.reject(new Error(message));
      return;
    }
    route.rustAttachment = undefined;
    attachment.dispose();
    retireActiveRoute(route, "rust-attachment-lost", () => {
      route.attachmentState = "closed";
    });
    route.providerProvisioningDispose?.();
    detachRouteAttachments(route, "rust-attachment-lost", new Error(message));
    if (win.isDestroyed() || win.webContents.isDestroyed()) return;
    win.webContents.send(PlatformChannels.RemoteSessionClosed, {
      sessionId,
      reason: "host-exit",
      exitCode: null,
      signal: null,
      error: message,
      reasonCode,
      retryable: true,
    });
    emitConnectionLog(win, { sessionId, level: "warn", message });
  }

  function retireActiveRoute(
    _route: RemoteAttachmentRoute,
    _reason: string,
    mutate: () => void,
  ): void {
    mutate();
  }

  function handleClosed(
    webContentsId: number,
    event: {
      remoteSessionId: string;
      reason: "connection-closed" | "disposed" | "connect-cancelled";
      exitCode?: number | null;
      signal?: string | null;
      error?: string;
    },
  ): void {
    const route = routesBySessionId.get(event.remoteSessionId);
    if (!route || route.webContentsId !== webContentsId) return;
    if (event.reason !== "connection-closed") {
      route.providerProvisioningDispose?.();
      retireActiveRoute(route, "disposed", () => {
        routesBySessionId.delete(event.remoteSessionId);
      });
      detachRouteAttachments(
        route,
        "session-released",
        new Error(`远程 workspace 已释放，sessionId=${event.remoteSessionId}`),
      );
      return;
    }
    retireActiveRoute(route, "connection-closed", () => {
      route.attachmentState = "closed";
    });
    route.providerProvisioningDispose?.();
    // 连接断开只把 route 标成 closed，已暴露的 desktop attachment 仍留在窗口 Host；
    // sessionId 换代后 Main 又会删除 route，导致旧 ChannelServer 永久失去回收入口。
    detachRouteAttachments(
      route,
      "session-connection-closed",
      new Error(`远程 workspace 已关闭，sessionId=${event.remoteSessionId}`),
    );
    const win = BrowserWindow.getAllWindows().find(
      (candidate) => candidate.webContents.id === webContentsId,
    );
    if (win && !win.isDestroyed() && !win.webContents.isDestroyed()) {
      win.webContents.send(PlatformChannels.RemoteSessionClosed, {
        sessionId: event.remoteSessionId,
        reason: "host-exit" as const,
        exitCode: event.exitCode ?? null,
        signal: event.signal ?? null,
      });
      emitConnectionLog(win, {
        sessionId: event.remoteSessionId,
        level: "warn",
        message: event.error || "远程 workspace 连接已断开",
      });
    }
  }

  function ensureHostListener(child: ElectronUtilityProcess, webContentsId: number): void {
    if (listenedHosts.has(child)) return;
    listenedHosts.add(child);
    child.on("message", (message: unknown) => {
      const parsed = hostResponseMessageSchema.safeParse(message);
      if (!parsed.success) return;
      if (parsed.data.type === HostResponseTypes.RemoteWorkspaceConnectionLog) {
        const pending = pendingByRequestKey.get(requestKey(webContentsId, parsed.data.requestId));
        if (!pending) return;
        // 多个远程连接共享 window Host，不能再从进程 label/stdout 猜日志归属；
        // Host 已带上连接 requestId，Main 只向对应发起窗口做薄转发。
        emitConnectionLog(pending.win, {
          requestId: parsed.data.requestId,
          level: parsed.data.level,
          message: parsed.data.message,
        });
        return;
      }
      if (parsed.data.type === HostResponseTypes.ProviderProvisioningSourceChanged) {
        void providerProvisioningCoordinator.requestAll(parsed.data.trigger);
        return;
      }
      if (parsed.data.type === HostResponseTypes.ProviderProvisioningExecutionResult) {
        const pending = pendingProviderProvisioningExecutions.get(parsed.data.requestId);
        if (!pending || pending.child !== child) return;
        pendingProviderProvisioningExecutions.delete(parsed.data.requestId);
        const logContext = {
          environmentKey: parsed.data.environmentKey,
          trigger: pending.trigger,
          status: parsed.data.status,
          durationMs: Math.max(0, monotonicNowMs() - pending.startedAtMonotonicMs),
        };
        if (parsed.data.status !== "applied" && parsed.data.status !== "already-applied") {
          const errorCode =
            parsed.data.errorCode ??
            (parsed.data.status === "unsupported"
              ? "capability-unavailable"
              : "target-apply-failed");
          // 首次失败以前在 warning 前返回，连接面板也只剩开始日志，无法区分本地与远端故障。
          // 仅记录经过 schema 校验的阶段码；远端 error 自由文本仍不进入日志或反馈。
          options.logger.warn("[provider-provisioning] Environment sync did not apply", {
            ...logContext,
            errorCode,
          });
          if (pending.trigger === "environment-online") {
            pending.reject(
              new Error(`Provider Provisioning 首次同步失败 (${parsed.data.status}; ${errorCode})`),
            );
            return;
          }
        } else {
          options.logger.info("[provider-provisioning] Environment sync completed", logContext);
        }
        pending.resolve();
        return;
      }
      if (parsed.data.type === HostResponseTypes.RemoteWorkspaceConnected) {
        handleConnected(child, webContentsId, parsed.data.requestId, parsed.data.descriptor);
        return;
      }
      if (parsed.data.type === HostResponseTypes.RemoteWorkspaceConnectFailed) {
        const key = requestKey(webContentsId, parsed.data.requestId);
        const pending = pendingByRequestKey.get(key);
        if (!pending) return;
        pendingByRequestKey.delete(key);
        emitConnectionLog(pending.win, {
          requestId: parsed.data.requestId,
          level: "error",
          message: parsed.data.error,
        });
        pending.reject(new Error(parsed.data.error));
        return;
      }
      if (parsed.data.type === HostResponseTypes.RemoteWorkspaceClosed) {
        handleClosed(webContentsId, parsed.data);
      }
    });
    child.once("exit", () => {
      const error = new Error("窗口 Local Host 已退出");
      for (const [requestId, pendingExecution] of pendingProviderProvisioningExecutions) {
        if (pendingExecution.child !== child) continue;
        pendingProviderProvisioningExecutions.delete(requestId);
        pendingExecution.reject(error);
      }
      for (const [key, pending] of Array.from(pendingByRequestKey)) {
        if (pending.webContentsId === webContentsId) {
          pendingByRequestKey.delete(key);
          pending.reject(error);
        }
      }
      for (const [sessionId, route] of Array.from(routesBySessionId)) {
        if (route.webContentsId !== webContentsId) continue;
        route.providerProvisioningDispose?.();
        retireActiveRoute(route, "host-exit", () => {
          routesBySessionId.delete(sessionId);
        });
        if (route.pendingRendererAttachment) {
          if (route.pendingRendererAttachment.timeout)
            clearTimeout(route.pendingRendererAttachment.timeout);
          route.pendingRendererAttachment.reject(error);
        }
      }
    });
  }

  async function createRemoteWorkspaceSession(
    win: BrowserWindow,
    target: RemoteTarget,
    requestId?: string,
    context?: RemoteWorkspaceSessionContext,
  ): Promise<string> {
    if (appShutdownStarted) {
      throw new Error("应用正在退出，无法创建远程工作区连接");
    }
    // renderer 的 requestId 同时用于 resolve 取消关联：首连在建时也能精确取消（spec §12.2）。
    const resolvedRequestId = requestId ?? randomUUID();
    // S5-a（spec §12.2）：ssh/wsl/docker 首连先由 Rust Host 建立/复用 Profile；建连事实与
    // 实例都在 Host，Main 只申请 scoped capability 并桥接 `/ws`。只有 `server` 保留 Node/HTTP。
    const resolved = await resolveRemoteTarget(target, {
      webContentsId: win.webContents.id,
      requestId: resolvedRequestId,
    });
    if (resolved.profileScope) {
      return createRustRemoteWorkspaceSession(
        win,
        resolved.target as ProfileRemoteTarget,
        resolved.profileScope,
        requestId,
        context,
      );
    }
    const child = await getWindowHost(win);
    const resolvedTarget = resolved.target;
    if (appShutdownStarted) {
      // WSL identity 解析跨 await，期间退出屏障可能已清空 Main 请求关联并开始回收 Host。
      // 恢复后必须重新验证生命周期，禁止在 shutdown barrier 之后注册迟到请求。
      throw new Error("应用正在退出，无法创建远程工作区连接");
    }
    if (win.isDestroyed() || win.webContents.isDestroyed()) {
      throw new Error("窗口已关闭，无法创建远程工作区连接");
    }
    const key = requestKey(win.webContents.id, resolvedRequestId);
    if (pendingByRequestKey.has(key)) {
      throw new Error(`远程连接 requestId 重复，requestId=${resolvedRequestId}`);
    }
    emitConnectionLog(win, {
      requestId: resolvedRequestId,
      level: "info",
      message: `正在通过窗口 Host 连接 ${resolvedTarget.kind} workspace`,
    });
    return new Promise<string>((resolve, reject) => {
      pendingByRequestKey.set(key, {
        requestId: resolvedRequestId,
        webContentsId: win.webContents.id,
        win,
        resolve,
        reject,
      });
      child.postMessage({
        type: HostMessageTypes.ConnectRemoteWorkspace,
        requestId: resolvedRequestId,
        target: resolvedTarget,
        remoteAssets: options.resolveRemoteAssetDirs(),
        ...(context?.workspacePath ? { workspacePath: context.workspacePath } : {}),
        ...(context?.workspaceIdentity ? { workspaceIdentity: context.workspaceIdentity } : {}),
      });
    });
  }

  /**
   * Rust Host 服务的远端 workspace session（spec §10.12）。
   *
   * 没有 per-window Node Host 参与：`remoteSessionId` 是 Main 的路由键，连接由 Host 按 profile 建立；
   * 首次 attach 会用临时 identity（`workspacePath ?? "/"`）申请 scoped capability，目录 bind 后
   * 换 new identity 重新签发（`RemoteGrant` 绑定签发时的 identity，不复用旧令牌）。
   */
  async function createRustRemoteWorkspaceSession(
    win: BrowserWindow,
    target: RemoteTarget,
    profileScope: { profileId: string; sourceServerId: string },
    requestId: string | undefined,
    context: RemoteWorkspaceSessionContext | undefined,
  ): Promise<string> {
    if (!options.attachRustRemoteScope) {
      throw Object.assign(
        new Error("当前 Host 不服务 Connection Profile 远端 workspace（Rust Host 未启用）"),
        { code: "REMOTE_RUST_HOST_UNAVAILABLE" as const },
      );
    }
    if (appShutdownStarted) {
      throw new Error("应用正在退出，无法创建远程工作区连接");
    }
    if (win.isDestroyed() || win.webContents.isDestroyed()) {
      throw new Error("窗口已关闭，无法创建远程工作区连接");
    }
    const resolvedRequestId = requestId ?? randomUUID();
    const key = requestKey(win.webContents.id, resolvedRequestId);
    if (pendingByRequestKey.has(key)) {
      throw new Error(`远程连接 requestId 重复，requestId=${resolvedRequestId}`);
    }
    // 与 Node Host 远程路径同一语义：目录未选定时用根路径占位，bind 时换代。
    const workspacePath =
      context?.workspacePath?.trim() ||
      (target.kind === "server" ? target.workspacePath?.trim() : undefined) ||
      "/";
    const workspaceIdentity =
      context?.workspaceIdentity?.trim() || buildRemoteWorkspaceIdentity(workspacePath, target);
    const remoteSessionId = randomUUID();
    const descriptor: WindowHostRemoteWorkspaceDescriptor = {
      remoteSessionId,
      target: stripRemoteTargetSecrets(target),
      workspacePath,
      workspaceIdentity,
      sourceServerId: profileScope.sourceServerId,
      ...(context?.workspacePurpose ? { workspacePurpose: context.workspacePurpose } : {}),
      generation: 1,
    };
    emitConnectionLog(win, {
      requestId: resolvedRequestId,
      level: "info",
      message: `正在通过 Rust Host 连接 ${target.kind} workspace（profile=${profileScope.profileId}）`,
    });
    return new Promise<string>((resolve, reject) => {
      const pending: PendingConnect = {
        requestId: resolvedRequestId,
        webContentsId: win.webContents.id,
        win,
        resolve,
        reject,
      };
      pendingByRequestKey.set(key, pending);
      const route: RemoteAttachmentRoute = {
        webContentsId: win.webContents.id,
        owner: "rust-host",
        descriptor,
        attachmentState: "attachable",
        connectedAtMonotonicMs: monotonicNowMs(),
        connectFinalized: false,
      };
      routesBySessionId.set(remoteSessionId, route);
      void attachRendererPort(win, route, "connect")
        .then(() => {
          pendingByRequestKey.delete(key);
          if (pending.cancelled) {
            throw Object.assign(new Error("远程连接已取消"), {
              code: "REMOTE_CONNECT_CANCELLED" as const,
            });
          }
          emitConnectionLog(win, {
            requestId: resolvedRequestId,
            sessionId: remoteSessionId,
            level: "info",
            message: `远程 ${target.kind} workspace 已通过 Rust Host 连接`,
          });
          resolve(remoteSessionId);
        })
        .catch((error: unknown) => {
          pendingByRequestKey.delete(key);
          routesBySessionId.delete(remoteSessionId);
          route.rustAttachment?.dispose();
          route.rustAttachment = undefined;
          emitConnectionLog(win, {
            requestId: resolvedRequestId,
            sessionId: remoteSessionId,
            level: "error",
            message: error instanceof Error ? error.message : String(error),
          });
          reject(error instanceof Error ? error : new Error(String(error)));
        });
    });
  }

  async function bindRemoteWorkspaceSessionContext(
    sessionId: string,
    context: RemoteWorkspaceSessionContext,
    expectedWebContentsId?: number,
  ): Promise<BindRemoteWorkspaceSessionContextResult> {
    const route = routesBySessionId.get(sessionId);
    if (!route) {
      if (expectedWebContentsId != null) {
        throw new Error(`未找到远程 workspace session，sessionId=${sessionId}`);
      }
      throw new Error(`未找到远程 workspace session，sessionId=${sessionId}`);
    }
    if (expectedWebContentsId != null && route.webContentsId !== expectedWebContentsId) {
      throw new Error(`远程 workspace session 不属于当前窗口，sessionId=${sessionId}`);
    }
    // Profile-scoped 远端 workspace 必须使用 Source Server 稳定身份（spec §5.1/§5.3）：
    // `remote:ssh:*` 只描述本次线路，重启/多窗口时无法归回同一 Source，启动重连也会
    // 因 `parseRemoteWorkspaceIdentity().kind !== "server"` 被跳过。
    const workspaceIdentity =
      context.workspaceIdentity?.trim() ||
      (route.descriptor.sourceServerId
        ? buildServerRemoteWorkspaceIdentity({
            serverId: route.descriptor.sourceServerId,
            workspacePath: context.workspacePath,
          })
        : buildRemoteWorkspaceIdentity(context.workspacePath, route.descriptor.target));
    // Renderer 在 bind 后只消费权威 scope；这里必须把 identity 一并回传，否则
    // 首次连接会以「bind 未返回权威 identity」失败并回收刚建立的 session。
    const previousWorkspaceIdentity = route.descriptor.workspaceIdentity?.trim();
    const workspacePurpose = context.workspacePurpose ?? route.descriptor.workspacePurpose;
    const binding: BindRemoteWorkspaceSessionContextResult = {
      remoteSessionId: sessionId,
      workspacePath: context.workspacePath,
      workspaceIdentity,
      ...(route.descriptor.sourceServerId
        ? { sourceServerId: route.descriptor.sourceServerId }
        : {}),
      ...(workspacePurpose ? { workspacePurpose } : {}),
      ...(previousWorkspaceIdentity && previousWorkspaceIdentity !== workspaceIdentity
        ? { previousWorkspaceIdentity }
        : {}),
    };
    const win = BrowserWindow.getAllWindows().find(
      (candidate) => candidate.webContents.id === route.webContentsId,
    );
    const child =
      route.owner === "node-host"
        ? options.windowHostProcessMap.get(route.webContentsId)
        : undefined;
    if ((route.owner === "node-host" && !child) || !win) {
      throw new Error(`未找到远程 workspace 所属窗口 Host，sessionId=${sessionId}`);
    }
    // Node Host：同一 parentPort 上 Bind 先于 Attach 处理。
    // Rust Host：scope 在 capability 签发时绑定 identity，bind 只需换代 attachment（下方 attach）。
    child?.postMessage({
      type: HostMessageTypes.BindRemoteWorkspaceContext,
      requestId: randomUUID(),
      remoteSessionId: sessionId,
      workspacePath: context.workspacePath,
      workspaceIdentity,
    });
    route.descriptor = {
      ...route.descriptor,
      workspacePath: context.workspacePath,
      workspaceIdentity,
      generation: route.descriptor.generation + 1,
    };
    // 同一 parentPort 上 Bind 先于 Attach 处理；IPC 只在 renderer 注册新 port 后返回。
    await attachRendererPort(win, route, "workspace-context-bound");
    return binding;
  }

  function reattachRemoteWorkspaceSessionsForWindow(win: BrowserWindow, reason: string): void {
    for (const route of routesBySessionId.values()) {
      if (route.webContentsId === win.webContents.id && route.attachmentState === "attachable") {
        void attachRendererPort(win, route, reason).catch((error: unknown) => {
          options.logger.warn("[window-host-remote] renderer reattach failed", {
            sessionId: route.descriptor.remoteSessionId,
            reason,
            error,
          });
        });
      }
    }
  }

  function disposeRemoteWorkspaceSession(sessionId: string, _reason?: string): void {
    const route = routesBySessionId.get(sessionId);
    if (!route) return;
    retireActiveRoute(route, "disposed", () => {
      routesBySessionId.delete(sessionId);
    });
    route.providerProvisioningDispose?.();
    const child =
      route.owner === "node-host"
        ? options.windowHostProcessMap.get(route.webContentsId)
        : undefined;
    if (route.pendingRendererAttachment) {
      const pending = route.pendingRendererAttachment;
      if (pending.timeout) clearTimeout(pending.timeout);
      route.pendingRendererAttachment = undefined;
      if (child) detachServicePort(child, pending.attachmentId, "session-disposed");
      pending.rustAttachment?.dispose();
      pending.reject(new Error(`远程 workspace session 已释放，sessionId=${sessionId}`));
    }
    if (route.owner === "rust-host") {
      // 远端连接与实例跨窗口共享（spec §10.6）：释放本窗口的 attachment，不断开 profile 连接。
      route.rustAttachment?.dispose();
      route.rustAttachment = undefined;
      return;
    }
    if (!child) return;
    if (route.rendererAttachmentId) {
      child.postMessage({
        type: HostMessageTypes.DetachServicePort,
        attachmentId: route.rendererAttachmentId,
      });
    }
    child.postMessage({
      type: HostMessageTypes.DisposeRemoteWorkspaceSession,
      requestId: randomUUID(),
      remoteSessionId: sessionId,
    });
  }

  /** 取消窗口/请求级首连等待者；多个 caller 共享同一 flight 时，只有最后一个离开才中止连接。 */
  function cancelPendingProfileResolutions(
    webContentsId: number,
    reason: string,
    requestId?: string,
  ): void {
    for (const pending of Array.from(pendingProfileResolutionCallers.values())) {
      if (
        pending.webContentsId !== webContentsId ||
        (requestId && pending.requestId !== requestId)
      ) {
        continue;
      }
      pending.cancel(reason || "远程连接已取消");
    }
  }

  function disposeRemoteWorkspaceSessionsForWindow(webContentsId: number): void {
    // 窗口在首连解析期间关闭：同样中止在途连接，避免 Host 里留下无人认领的 SSH socket。
    cancelPendingProfileResolutions(webContentsId, "窗口已关闭，远程连接已取消");
    for (const [sessionId, route] of Array.from(routesBySessionId)) {
      if (route.webContentsId !== webContentsId) continue;
      retireActiveRoute(route, "window-closed", () => {
        routesBySessionId.delete(sessionId);
      });
      route.providerProvisioningDispose?.();
      if (route.pendingRendererAttachment) {
        if (route.pendingRendererAttachment.timeout)
          clearTimeout(route.pendingRendererAttachment.timeout);
        route.pendingRendererAttachment.rustAttachment?.dispose();
        route.pendingRendererAttachment.reject(new Error("窗口已关闭，attachment 已取消"));
        route.pendingRendererAttachment = undefined;
      }
      route.rustAttachment?.dispose();
      route.rustAttachment = undefined;
    }
    for (const [key, pending] of Array.from(pendingByRequestKey)) {
      if (pending.webContentsId !== webContentsId) continue;
      pendingByRequestKey.delete(key);
      pending.cancelled = true;
      pending.reject(new Error("窗口已关闭，远程连接已取消"));
    }
  }

  function cancelPendingRemoteWorkspaceSessionsForWindow(
    webContentsId: number,
    reason: string,
    requestId?: string,
  ): void {
    // 首连解析阶段还没有 session/Node 请求；按 caller 单独取消，最后一个 caller 才中止 Host 连接。
    cancelPendingProfileResolutions(webContentsId, reason, requestId);
    const child = options.windowHostProcessMap.get(webContentsId);
    for (const pending of pendingByRequestKey.values()) {
      if (
        pending.webContentsId !== webContentsId ||
        (requestId && pending.requestId !== requestId)
      ) {
        continue;
      }
      // Rust Host 路径没有 per-request 控制消息：置取消位，迟到的 scope attachment 立即收口。
      pending.cancelled = true;
      child?.postMessage({
        type: HostMessageTypes.CancelRemoteWorkspaceConnect,
        requestId: pending.requestId,
      });
    }
  }

  function attachRemoteWorkspaceSessionHost(params: {
    windowId: number;
    remoteSessionId: string;
    workspacePath: string;
    workspaceIdentity: string;
    workspaceKey: string;
    clientMode: "web-remote-replayable";
  }): {
    process: ElectronUtilityProcess;
    port: MessagePortMain;
    remoteKind: RemoteTarget["kind"];
  } {
    const route = routesBySessionId.get(params.remoteSessionId);
    if (!route) {
      throw Object.assign(
        new Error(`未找到远程 workspace session，sessionId=${params.remoteSessionId}`),
        {
          code: "REMOTE_SESSION_MISSING" as const,
        },
      );
    }
    if (route.attachmentState !== "attachable") {
      throw Object.assign(new Error("远程 workspace source 当前离线"), {
        code: "REMOTE_SESSION_OFFLINE" as const,
      });
    }
    if (route.owner !== "node-host" || route.descriptor.target.kind !== "server") {
      throw Object.assign(new Error("当前 renderer remote-host attachment 只服务 Web 服务端目标"), {
        code: "REMOTE_RUST_HOST_REQUIRED" as const,
      });
    }
    const win = BrowserWindow.fromId(params.windowId);
    if (!win || win.webContents.id !== route.webContentsId) {
      throw Object.assign(new Error("远程 workspace session 不属于当前窗口"), {
        code: "REMOTE_SESSION_WINDOW_MISMATCH" as const,
      });
    }
    const descriptor = route.descriptor;
    if (
      descriptor.workspacePath !== params.workspacePath ||
      descriptor.workspaceIdentity !== params.workspaceIdentity ||
      params.workspaceKey !== params.workspaceIdentity
    ) {
      throw Object.assign(new Error("远程 workspaceKey 与 logical session 不匹配。"), {
        code: "REMOTE_WORKSPACE_IDENTITY_MISMATCH" as const,
      });
    }
    const process = getWindowHostSync(win);
    const { port1, port2 } = createMessageChannel();
    process.postMessage(
      {
        type: HostMessageTypes.AttachServicePort,
        requestId: randomUUID(),
        attachmentId: randomUUID(),
        clientMode: params.clientMode,
        scope: {
          kind: "remote",
          remoteSessionId: params.remoteSessionId,
          workspacePath: params.workspacePath,
          workspaceIdentity: params.workspaceIdentity,
        },
      },
      [port2],
    );
    return { process, port: port1, remoteKind: descriptor.target.kind };
  }

  return {
    createRemoteWorkspaceSession,
    attachRemoteWorkspaceSessionHost,
    bindRemoteWorkspaceSessionContext,
    confirmRendererAttachmentReady,
    reattachRemoteWorkspaceSessionsForWindow,
    disposeRemoteWorkspaceSession,
    disposeRemoteWorkspaceSessionsForWindow,
    disposeAllAndWaitForAppShutdown: async (_reason: string) => {
      appShutdownStarted = true;
      const error = new Error("应用正在退出，远程连接已取消");
      for (const pending of pendingByRequestKey.values()) pending.reject(error);
      pendingByRequestKey.clear();
      for (const [sessionId, route] of Array.from(routesBySessionId)) {
        retireActiveRoute(route, "app-shutdown", () => {
          routesBySessionId.delete(sessionId);
        });
        route.providerProvisioningDispose?.();
        if (!route.pendingRendererAttachment) continue;
        if (route.pendingRendererAttachment.timeout)
          clearTimeout(route.pendingRendererAttachment.timeout);
        route.pendingRendererAttachment.reject(error);
      }
      routesBySessionId.clear();
    },
    cancelPendingRemoteWorkspaceSessionsForWindow,
    handleWorkspaceRunningTaskCountChanged: () => {
      // Running-task 事实现在由窗口 Host registry/ControllerProjection 持有；Main 不再维护 WSL pool。
    },
  };
}
