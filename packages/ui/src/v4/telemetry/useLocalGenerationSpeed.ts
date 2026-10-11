import { useCallback, useSyncExternalStore } from "react";
import {
  getLocalGenerationSpeedTracker,
  NO_GENERATION_SPEED,
  type GenerationSpeedView,
} from "@/v4/telemetry/localGenerationSpeed.js";

const NOOP_SUBSCRIBE = () => () => {};
const READ_NO_SPEED = () => NO_GENERATION_SPEED;

/**
 * 订阅某会话的生成速度视图。
 *
 * 组件自订阅而不是由 Composer 传值：流式期间读数每 500ms 变一次，走 props 会把 Composer 控制簇的
 * memo 打穿（见 ConversationComposer 中 submitControlNode 的注释）。
 */
export function useLocalGenerationSpeed(sessionId: string | null): GenerationSpeedView {
  const key = sessionId ?? "";
  const subscribe = useCallback(
    (onStoreChange: () => void) =>
      key ? getLocalGenerationSpeedTracker().subscribe(key, onStoreChange) : NOOP_SUBSCRIBE(),
    [key],
  );
  const read = useCallback(
    () => (key ? getLocalGenerationSpeedTracker().read(key) : NO_GENERATION_SPEED),
    [key],
  );
  // 第三参数固定返回空视图：服务端渲染没有帧流，不能凭空造读数。
  return useSyncExternalStore(subscribe, read, READ_NO_SPEED);
}
