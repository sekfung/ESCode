import { downloadESCodeBuiltinRelease, type ESCodeBuiltinRelease } from "@escode/provider-node";
import type { ApiClient } from "@escode/shared";

interface FetchESCodeBuiltinRemoteReleaseOptions {
  readonly apiClient: ApiClient;
  readonly endpointOrigin: string;
  readonly appVersion: string;
  readonly platform: string;
  readonly signal?: AbortSignal;
}

/** Services 仅注入既有网络装配；URL、预算与 Release 校验由 provider-node 唯一实现。 */
export async function fetchESCodeBuiltinRemoteRelease(
  options: FetchESCodeBuiltinRemoteReleaseOptions,
): Promise<ESCodeBuiltinRelease | null> {
  return downloadESCodeBuiltinRelease({
    endpointOrigin: options.endpointOrigin,
    appVersion: options.appVersion,
    platform: options.platform,
    signal: options.signal,
    request: (url, init) => options.apiClient.request(url, init),
  });
}
