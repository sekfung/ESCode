import {
  escodeProtocolMethods,
  escodePluginsReferenceCatalogResultSchema,
  type ESCodePluginsReferenceCatalogParams,
} from "@escode/shared";
import type { ESCodeProtocolClient } from "#src/escode-agent/escodeProtocolClient.js";

/** 旧协议严格校验响应；新展示字段走独立入口，只有 -32601 能证明旧 Agent 不支持。 */
export async function requestPluginReferenceCatalog(
  client: Pick<ESCodeProtocolClient, "request">,
  params: ESCodePluginsReferenceCatalogParams,
) {
  try {
    return await client.request(
      escodeProtocolMethods.pluginsReferenceCatalogWithCategory,
      params,
      escodePluginsReferenceCatalogResultSchema,
    );
  } catch (error) {
    if (!(typeof error === "object" && error !== null && "code" in error && error.code === -32601))
      throw error;
    return client.request(
      escodeProtocolMethods.pluginsReferenceCatalog,
      params,
      escodePluginsReferenceCatalogResultSchema,
    );
  }
}
