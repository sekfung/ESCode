import { pluginUiStateKey } from "@/plugin-ui/adapters/pluginUiStateKey.js";
import { logger } from "@/logger.js";
import {
  clearPluginUiWidgetState,
  getPluginUiWidgetState,
  setPluginUiWidgetState,
} from "@/plugin-ui/app/pluginUiWidgetStateStore.js";
import type { PluginSandboxHandle } from "@escode/shared/mcp-apps";
import { buildPluginSandboxScopeId } from "@escode/shared/mcp-apps";
import { clearPluginUiSurfaceFeed } from "../app/pluginUiSurfaceFeedStore.js";
import { PLUGIN_UI_CARD_DEFAULT_HEIGHT_PX } from "../contract.js";
import { buildPluginUiHostContext } from "../domain/buildPluginUiHostContext.js";
import { readPluginUiStyleVariables } from "./readPluginUiStyleVariables.js";

import { acquireWorkspaceConnection } from "@/v4/workspaceConnectionRegistry.js";
import type { IServiceAccessor } from "@escode/services";
import type { IPlatformService } from "@escode/shared";
import { buildPluginUiSessionKey } from "../contract.js";
import type { Page, UsePluginUiHostInput, UsePluginUiHostResult } from "./pluginUiHostTypes.js";
import { createPluginUiPagePlane } from "./pluginUiPagePlane.js";
import { pluginUiPages } from "./pluginUiPageRegistry.js";
import { attachPluginUiResourceDeltaFeed } from "./pluginUiResourceDeltaFeed.js";
import { createManagedPageController } from "./pluginUiManagedPageController.js";
import type {
  PluginUiHostController,
  PluginUiHostPhase,
} from "@/plugin-ui/app/pluginUiHostController.js";
import type { McpUiAppCapabilities } from "@modelcontextprotocol/ext-apps/app-bridge";
type Platform = IPlatformService;
type Services = IServiceAccessor;
export function createPage(
  key: string,
  input: UsePluginUiHostInput,
  platform: Platform,
  services: Services,
): Page {
  const bridge = services.pluginUiBridgeService!;
  const sandboxPlatform = platform.pluginSandbox!;
  const surfaceKey = key;
  const latest = { current: input };
  const listeners = new Set<() => void>();
  let surfaceStateKey: string | undefined;
  let controller: PluginUiHostController | null = null;
  let releaseFeed: (() => void) | undefined;
  let releaseLease: (() => void) | undefined;
  let executionKey = "";
  let disposed = false;
  let starting = false;
  let incarnation = 0;
  let releasing: Promise<unknown> = Promise.resolve();
  let contextInputs = "";
  let lastFeed = "";
  let lastContext = "";
  let appIdentity: string | undefined;
  const notify = () => {
    for (const listener of listeners) listener();
  };
  let snapshot: UsePluginUiHostResult = {
    pageKey: key,
    supported: true,
    phase: "idle",
    handle: null,
    height: PLUGIN_UI_CARD_DEFAULT_HEIGHT_PX,
    error: null,
    appCapabilities: undefined,
    reload: () => {
      clearPluginUiWidgetState(key);
      void page.suspend(true).then(ensure);
    },
  };
  const patch = (value: Partial<UsePluginUiHostResult>) => {
    snapshot = { ...snapshot, ...value };
    notify();
  };
  const setPhase = (phase: PluginUiHostPhase) => patch({ phase });
  const setHandle = (handle: PluginSandboxHandle) => {
    if (appIdentity && appIdentity !== handle.instance.appIdentity) clearPluginUiWidgetState(key);
    appIdentity = handle.instance.appIdentity;
    patch({ handle });
    plane.mount(handle);
    syncExecution();
  };
  const setHeight = (height: number) => patch({ height });
  const setError = (error: string) => patch({ error });
  const setAppCapabilities = (appCapabilities: McpUiAppCapabilities | undefined) =>
    patch({ appCapabilities });
  const plane = createPluginUiPagePlane({
    onVisibility(visible) {
      if (page.visible !== visible) pluginUiPages.visibility(page, visible);
      if (visible) ensure();
      syncExecution();
    },
    onCrash() {
      void page.suspend(true);
      patch({ phase: "error", error: "MCP App page stopped" });
    },
  });
  function syncExecution() {
    // 握手完成和 session 空闲都不能证明插件内部已无工作；宿主保留页面时必须允许它执行。
    const active = controller !== null;
    plane.setExecutionActive(active);
    const handle = snapshot.handle;
    if (!handle || !sandboxPlatform.setExecutionActive) return;
    const background = active && (!page.visible || document.hidden);
    const nextKey = `${handle.sandboxId}:${handle.initId}:${background}`;
    if (nextKey === executionKey) return;
    executionKey = nextKey;
    logger.lifecycle.info("[plugin-ui] background execution", {
      sessionId: input.scope.sessionId,
      sandboxId: handle.sandboxId,
      active: background,
    });
    void sandboxPlatform
      .setExecutionActive(handle.sandboxId, handle.initId, background)
      .catch((error: unknown) => {
        if (snapshot.handle !== handle || disposed) return;
        logger.warn("[plugin-ui] background execution failed", { error: String(error) });
      });
  }
  async function ensure() {
    if (disposed || controller || starting || snapshot.phase === "error") return;
    starting = true;
    const epoch = ++incarnation;
    try {
      await releasing;
      surfaceStateKey = await pluginUiStateKey(surfaceKey);
      if (disposed || epoch !== incarnation) return;
      await pluginUiPages.reserve(page);
    } catch (error) {
      starting = false;
      patch({ phase: "error", error: String(error) });
      return;
    }
    if (disposed || epoch !== incarnation) {
      pluginUiPages.released(page);
      return;
    }
    starting = false;
    try {
      // 页面独立持有已有 workspace 连接租约；卡片或 pane 退出不会让后台页面失联。
      const connection = acquireWorkspaceConnection(
        latest.current.scope,
        services.escodeAgentService,
        platform.createLocalMediaPreviewUrl,
      );
      releaseLease = () => connection.release();
      const lease = connection.layer.acquire(latest.current.scope.sessionId);
      const runtimeStopped = () => {
        void page.suspend(true);
        patch({ phase: "error", error: "MCP App runtime is no longer available" });
      };
      const offRestart = connection.transport.onRuntimeRestart(runtimeStopped);
      const offLifecycle = connection.transport.onRuntimeLifecycle?.((state) => {
        if (state === "unavailable") runtimeStopped();
      });
      releaseLease = () => {
        offRestart();
        offLifecycle?.();
        lease.release();
        connection.release();
      };
      releaseFeed = attachPluginUiResourceDeltaFeed(lease.store, latest.current.scope);
      patch({ phase: "preparing", error: null, handle: null });
      createController();
    } catch (error) {
      void page.suspend(true);
      patch({ phase: "error", error: String(error) });
    }
  }
  function createController() {
    const nextController = createManagedPageController({
      bridge,
      sandboxPlatform,
      services,
      platform,
      surfaceKey,
      latest,
      page,
      getSnapshot: () => snapshot,
      getSurfaceStateKey: () => surfaceStateKey,
      getController: () => controller,
      patch,
      setPhase,
      setHandle,
      setHeight,
      setError,
      setAppCapabilities,
    });
    controller = nextController;
    controller.start();
  }
  const task = buildPluginUiSessionKey(input.scope);
  const workspaceEvents = services.escodeTaskService.onDynamicWorkspaceEvent({
    workspacePath: input.scope.workspacePath,
    workspaceIdentity: input.scope.workspaceIdentity,
  })((event) => {
    if (
      event.type === "workspace_task_list_changed" &&
      event.reason === "task_deleted" &&
      event.taskId === input.scope.sessionId
    )
      pluginUiPages.removeTask(task);
  });
  const page: Page = {
    kind: "mcp",
    key,
    task,
    visible: false,
    snapshot: () => snapshot,
    start: () => void ensure(),
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    busy: () => controller?.isBusy() ?? false,
    running: () => controller !== null || starting,
    bind(node, sidebar) {
      const off = plane.bind({
        node,
        kind: sidebar ? "sidebar" : "inline",
        container: node.closest<HTMLElement>("[data-sandbox-page-container]") ?? undefined,
      });
      ensure();
      return off;
    },
    update(next) {
      // 背景内联锚点不能覆盖侧栏当前的交互上下文。
      if (next.enabled === false) return;
      latest.current = next;
      const nextContextInputs = JSON.stringify([
        next.theme,
        next.locale,
        next.displayMode,
        next.width,
        next.titleBar,
        next.availableDisplayModes,
      ]);
      if (nextContextInputs !== contextInputs) {
        contextInputs = nextContextInputs;
        const context = buildPluginUiHostContext({
          theme: next.theme,
          locale: next.locale,
          displayMode: next.displayMode,
          width: next.width,
          titleBar: next.titleBar,
          availableDisplayModes: next.availableDisplayModes,
          styleVariables: readPluginUiStyleVariables(),
        });
        const contextKey = JSON.stringify(context);
        if (contextKey !== lastContext) {
          lastContext = contextKey;
          controller?.updateHostContext(context);
        }
      }
      page.updateToolFeed(next);
    },
    updateToolFeed(next) {
      if (disposed) return;
      latest.current = {
        ...latest.current,
        toolCallId: next.toolCallId,
        toolInput: next.toolInput,
        toolResult: next.toolResult,
        toolCancelled: next.toolCancelled,
      };
      const feed = JSON.stringify([
        next.toolCallId,
        next.toolInput,
        next.toolResult,
        next.toolCancelled,
      ]);
      if (feed !== lastFeed) {
        lastFeed = feed;
        if (next.toolInput) controller?.notifyToolInput({ arguments: next.toolInput });
        if (next.toolResult) controller?.notifyToolResult(next.toolResult);
        if (next.toolCancelled) controller?.notifyToolCancelled();
      }
    },
    async suspend(force = false) {
      // 编辑器的未保存文档在 guest 内存中；即使 session 空闲，也不能按离屏期限或容量回收。
      // 管理器沿用 false 拒绝淘汰；删除、重载与来源失效仍显式 force 清理。
      if (!force) return false;
      incarnation++;
      starting = false;
      const previous = controller;
      controller = null;
      plane.setExecutionActive(false);
      executionKey = "";
      const previousHandle = snapshot.handle;
      const offFeed = releaseFeed,
        offLease = releaseLease;
      releaseFeed = undefined;
      releaseLease = undefined;
      patch({ phase: "idle", handle: null, appCapabilities: undefined });
      // 先前释放未完成时不能重新预备；反复重试也只等待同一个释放链。
      // Main 主动导航到 about:blank 会触发 navigation；先解除旧代际观察，避免回收被误报为崩溃。
      if (previousHandle) plane.prepareUnmount(previousHandle.sandboxId);
      const closing = previous?.dispose();
      releasing = Promise.all([releasing, closing]).then(() => {
        if (previousHandle) plane.unmount(previousHandle.sandboxId);
        offFeed?.();
        offLease?.();
        pluginUiPages.released(page);
      });
      await releasing;
      return true;
    },
    destroy() {
      document.removeEventListener("visibilitychange", syncExecution);
      workspaceEvents.dispose();
      clearPluginUiSurfaceFeed(key);
      disposed = true;
      void page.suspend(true).finally(() => plane.dispose());
      clearPluginUiWidgetState(key);
      listeners.clear();
    },
  };
  document.addEventListener("visibilitychange", syncExecution);
  return page;
}
