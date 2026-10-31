/* eslint-disable max-lines -- Main 的完整薄转发契约集中维护请求与端口关联。 */
import { randomUUID } from "node:crypto";
import { BrowserWindow } from "electron";
import type { MessagePortMain } from "electron";
import {
  type BindRemoteWorkspaceSessionContextResult,
  buildRemoteWorkspaceIdentity,
  buildServerRemoteWorkspaceIdentity,
  InternalChannels,
  PlatformChannels,
  type RemoteTarget,
  type ScopedServicePortPayload,
  type WorkspacePurpose,
  type WindowHostRemoteWorkspaceDescriptor,
  stripRemoteTargetSecrets,
} from "@escode/shared";
import type { RustHostRemoteScope } from "./rustHostBridge.js";

interface RemoteWorkspaceSessionContext {
  workspacePath: string;
  workspaceIdentity?: string;
  workspacePurpose?: WorkspacePurpose;
}

interface ResolvedProfileScope {
  profileId: string;
  sourceServerId: string;
}

interface ResolvedRemoteTarget {
  target: RemoteTarget;
  profileScope: ResolvedProfileScope;
}

/** 首连 single-flight 键：与 Rust 的 Profile 身份键一致，不含密码/口令/展示字段。 */
function profileTargetIdentityKey(target: RemoteTarget): string {
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
    // spec §12.6.2：server 身份只看 trim 后的 URL，不做 http/ws 协议互转。
    case "server":
      return JSON.stringify({ kind: "server", url: target.url.trim() });
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
  /** Rust Host 的 `/ws` 桥接句柄（dispose 即关闭连接与端口）。 */
  rustAttachment?: RustScopeAttachment;
  // Main 只保留端口转发所需的 request/session 关联和脱敏 descriptor；连接/任务事实归 Host。
  descriptor: WindowHostRemoteWorkspaceDescriptor;
  rendererAttachmentId?: string;
  pendingRendererAttachment?: {
    attachmentId: string;
    reason: string;
    /** 候选 attachment 的桥接句柄（ready 后提升为 `rustAttachment`）。 */
    rustAttachment?: RustScopeAttachment;
    /** 附件（capability/安装）就绪、端口交付后才开始计时。 */
    timeout?: NodeJS.Timeout;
    resolve: () => void;
    reject: (error: Error) => void;
  };
  attachmentState: "attachable" | "closed";
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

export function createRemoteWorkspaceSessionManager(options: {
  logger: {
    info: (...args: unknown[]) => void;
    warn: (...args: unknown[]) => void;
    error: (...args: unknown[]) => void;
  };
  resolveWslTarget?: (
    target: Extract<RemoteTarget, { kind: "wsl" }>,
  ) => Promise<Extract<RemoteTarget, { kind: "wsl" }>>;
  /**
   * 首连解析（S5-a，spec §12.2；S5-f-4 起含 `kind: "server"`）：无 Profile 归属的目标交给 Rust Host
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
}) {
  const pendingByRequestKey = new Map<string, PendingConnect>();
  const routesBySessionId = new Map<string, RemoteAttachmentRoute>();
  let appShutdownStarted = false;
  const pendingProfileResolutions = new Map<string, RemoteProfileResolutionFlight>();
  const pendingProfileResolutionCallers = new Map<string, PendingProfileResolution>();

  /**
   * 所有远程目标（含 `kind: "server"`，S5-f-4）都由 Rust Host 服务。已有完整 Profile 归属时直接
   * 路由；首连或历史快照缺字段时，由 Rust Host 创建/匹配 Profile 并回填 `sourceServerId`。
   */
  async function resolveRemoteTarget(
    target: RemoteTarget,
    caller: { webContentsId: number; requestId: string },
  ): Promise<ResolvedRemoteTarget> {
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
      ...(profileTarget as unknown as RemoteTarget),
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
    target: RemoteTarget,
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
    const profileId = descriptor.target.profileId;
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
   * 把一个 route 的 attachment 交付给 renderer（S3-c，spec §10.12）：向 Rust Host 申请 scoped
   * capability 并桥接 `/ws`；失败是显式错误（不换其它 Host 服务同一 workspace），候选 attachment
   * 立即收口。
   */
  async function attachRendererPort(
    win: BrowserWindow,
    route: RemoteAttachmentRoute,
    reason: string,
  ): Promise<void> {
    if (win.isDestroyed() || win.webContents.isDestroyed()) {
      throw new Error("窗口已关闭，无法 attachment 远程 workspace");
    }
    const descriptor = route.descriptor;
    const workspacePath = descriptor.workspacePath;
    const workspaceIdentity = descriptor.workspaceIdentity;
    if (!workspacePath || !workspaceIdentity) {
      throw new Error(
        `远程 descriptor 缺少 workspace scope，sessionId=${descriptor.remoteSessionId}`,
      );
    }
    // scope 先解析：缺字段属于契约错误，不应先建端口再失败。
    const rustScope = remoteScopeOf(route);
    const attachmentId = randomUUID();
    const scopedServicePort = buildScopedServicePortPayload(descriptor, attachmentId, {
      workspacePath,
      workspaceIdentity,
    });
    const superseded = route.pendingRendererAttachment;
    if (superseded) {
      if (superseded.timeout) clearTimeout(superseded.timeout);
      route.pendingRendererAttachment = undefined;
      superseded.rustAttachment?.dispose();
      superseded.reject(
        new Error(`renderer attachment 已被后续换代替代，sessionId=${descriptor.remoteSessionId}`),
      );
    }

    return new Promise<void>((resolve, reject) => {
      // renderer attachment ready 只覆盖「端口送达 → renderer 注册完服务」这段握手。
      // 端口要等 Host 建连（首次连接含安装，spec §11.3）后才能交付，因此计时器在附件就绪后
      // 才启动；连接/安装进度由 Source Server 状态机发布。
      const armReadyTimeout = (): NodeJS.Timeout => {
        const timer = setTimeout(() => {
          const pending = route.pendingRendererAttachment;
          if (!pending || pending.attachmentId !== attachmentId) return;
          route.pendingRendererAttachment = undefined;
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
      route.pendingRendererAttachment = { attachmentId, reason, resolve, reject };

      const attachRustRemoteScope = options.attachRustRemoteScope;
      if (!attachRustRemoteScope) {
        route.pendingRendererAttachment = undefined;
        reject(
          Object.assign(
            new Error("当前 Host 不服务 Connection Profile 远端 workspace（Rust Host 未启用）"),
            { code: "REMOTE_RUST_HOST_UNAVAILABLE" as const },
          ),
        );
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
          attachment.onLost?.((reason) => handleRustAttachmentLost(win, route, attachment, reason));
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
    route.rendererAttachmentId = pending.attachmentId;
    // 候选 attachment 就绪后才淘汰旧句柄：换代期间 renderer 仍可用旧 port。
    route.rustAttachment?.dispose();
    route.rustAttachment = pending.rustAttachment;
    options.logger.info(
      `[window-host-remote] renderer attachment ready, sessionId=${payload.sessionId}, reason=${pending.reason}`,
    );
    pending.resolve();
  }

  /** 连接/窗口关闭时收口端口：候选与已交付的 attachment 都要 dispose，避免 `/ws` 泄漏。 */
  function detachRouteAttachments(route: RemoteAttachmentRoute, error: Error): void {
    if (route.pendingRendererAttachment) {
      const pending = route.pendingRendererAttachment;
      if (pending.timeout) clearTimeout(pending.timeout);
      route.pendingRendererAttachment = undefined;
      pending.rustAttachment?.dispose();
      pending.reject(error);
    }
    if (route.rendererAttachmentId) {
      route.rustAttachment?.dispose();
      route.rustAttachment = undefined;
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
    detachRouteAttachments(route, new Error(message));
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
    // S5-a/S5-f-4（spec §12.2/§12.6）：所有 target（含 `kind: "server"`）先由 Rust Host 建立/复用
    // Profile；建连事实与实例都在 Host，Main 只申请 scoped capability 并桥接 `/ws`。
    const resolved = await resolveRemoteTarget(target, {
      webContentsId: win.webContents.id,
      requestId: resolvedRequestId,
    });
    if (appShutdownStarted) {
      // WSL identity 解析跨 await，期间退出屏障可能已清空 Main 请求关联并开始回收 Host。
      // 恢复后必须重新验证生命周期，禁止在 shutdown barrier 之后注册迟到请求。
      throw new Error("应用正在退出，无法创建远程工作区连接");
    }
    if (win.isDestroyed() || win.webContents.isDestroyed()) {
      throw new Error("窗口已关闭，无法创建远程工作区连接");
    }
    return createRustRemoteWorkspaceSession(
      win,
      resolved.target,
      resolved.profileScope,
      requestId,
      context,
    );
  }

  /**
   * Rust Host 服务的远端 workspace session（spec §10.12）。
   *
   * 不再有 per-window Node Host 参与：`remoteSessionId` 是 Main 的路由键，连接由 Host 按 profile 建立；
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
        descriptor,
        attachmentState: "attachable",
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
    if (!win) {
      throw new Error(`未找到远程 workspace 所属窗口，sessionId=${sessionId}`);
    }
    // Rust Host：scope 在 capability 签发时绑定 identity，bind 只需换代 attachment（下方 attach）。
    route.descriptor = {
      ...route.descriptor,
      workspacePath: context.workspacePath,
      workspaceIdentity,
      generation: route.descriptor.generation + 1,
    };
    // IPC 只在 renderer 注册新 port 后返回。
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
    if (route.pendingRendererAttachment) {
      const pending = route.pendingRendererAttachment;
      if (pending.timeout) clearTimeout(pending.timeout);
      route.pendingRendererAttachment = undefined;
      pending.rustAttachment?.dispose();
      pending.reject(new Error(`远程 workspace session 已释放，sessionId=${sessionId}`));
    }
    // 远端连接与实例跨窗口共享（spec §10.6）：释放本窗口的 attachment，不断开 profile 连接。
    route.rustAttachment?.dispose();
    route.rustAttachment = undefined;
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
    // 首连解析阶段还没有 session；按 caller 单独取消，最后一个 caller 才中止 Host 连接。
    cancelPendingProfileResolutions(webContentsId, reason, requestId);
    for (const pending of pendingByRequestKey.values()) {
      if (
        pending.webContentsId !== webContentsId ||
        (requestId && pending.requestId !== requestId)
      ) {
        continue;
      }
      // Rust Host 路径没有 per-request 控制消息：置取消位，迟到的 scope attachment 立即收口。
      pending.cancelled = true;
    }
  }

  return {
    createRemoteWorkspaceSession,
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
