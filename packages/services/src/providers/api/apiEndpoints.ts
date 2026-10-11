import { buildRuntimeESCodeApiUrl, resolveZaiBusinessBaseUrl } from "@escode/shared";

export const ESCODE_CLIENT_SCENES_URL = buildRuntimeESCodeApiUrl(
  process.env,
  "/api/v1/client/scenes",
);

export const ZAI_API_HOST = resolveZaiBusinessBaseUrl(process.env);
