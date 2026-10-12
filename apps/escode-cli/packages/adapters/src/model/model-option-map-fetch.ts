import type {
  CompiledModelOptionMaps,
  JsonObject,
  ModelOptionValues,
} from "@escode/model-option-map";

type ProviderFetch = typeof globalThis.fetch;

export interface RawRequestBodyCapture {
  body?: JsonObject;
}

export function createModelOptionMapFetch(input: {
  readonly capture?: RawRequestBodyCapture;
  readonly fetch: ProviderFetch;
  readonly maps: CompiledModelOptionMaps;
  readonly values: ModelOptionValues;
  /** 打补丁之后、捕获与发送之前，对最终正文做协议补齐（如 Anthropic thinking 预算）。 */
  readonly finalizeBody?: (body: JsonObject) => JsonObject;
}): ProviderFetch {
  return async (request, init) => {
    const bodyText = await readRequestBody(request, init);
    if (bodyText === undefined) return input.fetch(request, init);
    const body = parseJsonObject(bodyText);
    const patched = input.maps.apply(body, input.values);
    const finalized = input.finalizeBody ? input.finalizeBody(patched) : patched;
    if (input.capture) input.capture.body = finalized;
    const patchedBody = JSON.stringify(finalized);
    if (request instanceof Request) {
      return input.fetch(new Request(request, { ...init, body: patchedBody }));
    }
    return input.fetch(request, { ...init, body: patchedBody });
  };
}

async function readRequestBody(
  request: RequestInfo | URL,
  init: RequestInit | undefined,
): Promise<string | undefined> {
  if (typeof init?.body === "string") return init.body;
  // Option Map 是 reasoning/max-output 的唯一请求字段权威。若 SDK 改成非文本 Body 却静默
  // 跳过 Patch，请求仍会发出但丢失两个 Option；因此有 Body 时必须 fail-closed。
  if (init?.body !== undefined && init.body !== null) {
    throw new Error("Model option maps require a JSON text request body.");
  }
  if (request instanceof Request) return request.clone().text();
  return undefined;
}

function parseJsonObject(body: string): JsonObject {
  const parsed: unknown = JSON.parse(body);
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("Model option maps require a JSON object request body.");
  }
  return parsed as JsonObject;
}
