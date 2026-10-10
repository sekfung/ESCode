import type { BrowserClientTransport } from "@escode/core/browser-client";

export const NODE_REPL_BROWSER_BRIDGE_SYMBOL = Symbol.for("escode.node-repl.browser-control-bridge");
export const BROWSER_UNAVAILABLE_IN_SUBAGENT_MESSAGE = "Browser is not available in subagent";

export interface NodeReplBrowserRuntimeBridge extends BrowserClientTransport {
  documentationRoot: string;
  assertAvailable(): void;
}

export function readNodeReplBrowserRuntimeBridge(
  globals: Record<PropertyKey, unknown>,
): NodeReplBrowserRuntimeBridge {
  const bridge = globals[NODE_REPL_BROWSER_BRIDGE_SYMBOL];
  if (!bridge || typeof bridge !== "object") {
    throw new Error(
      "Browser runtime bridge is unavailable. Use Browser from an ESCode desktop or shared-host session.",
    );
  }
  return bridge as NodeReplBrowserRuntimeBridge;
}
