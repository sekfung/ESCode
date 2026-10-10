import {
  buildRuntimeESCodeEndpointUrls,
  ESCODE_ENV,
  type RuntimeESCodeEndpointEnv,
} from "@escode/shared";

interface RendererImportMetaEnv {
  VITE_ESCODE_BASE_URL?: string;
  VITE_ESCODE_ENDPOINT_ORIGIN?: string;
}

function readRendererImportMetaEnv(): RendererImportMetaEnv {
  return ((import.meta as ImportMeta & { env?: RendererImportMetaEnv }).env ??
    {}) as RendererImportMetaEnv;
}

function createRendererESCodeEndpointEnv(
  env: RendererImportMetaEnv = readRendererImportMetaEnv(),
): RuntimeESCodeEndpointEnv {
  return {
    ESCODE_ENV,
    // UI 侧的 escode-plan 占位 provider 以前只看 ESCODE_ENV，
    // 没有消费 Vite 注入的 base url，导致自定义测试域名时 renderer 和 host/service 可能不一致。
    ESCODE_BASE_URL: env.VITE_ESCODE_BASE_URL,
    ESCODE_ENDPOINT_ORIGIN: env.VITE_ESCODE_ENDPOINT_ORIGIN,
  };
}

export const RENDERER_ESCODE_ENDPOINT_URLS = buildRuntimeESCodeEndpointUrls(
  createRendererESCodeEndpointEnv(),
);
