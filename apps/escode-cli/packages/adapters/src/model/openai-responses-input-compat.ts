type ProviderFetch = typeof globalThis.fetch;

/**
 * OpenAI Responses input item 形态归一化。
 *
 * 规范里 `output_text` 只合法于带 `id`/`status` 的 `ResponseOutputMessage` item；easy message 的
 * content 列表只接受 `input_text` 等 `input_*` 类型。`@ai-sdk/openai` 会把 assistant 文本发成
 * `{ role: "assistant", content: [{ type: "output_text", text }] }`，OpenAI 官方服务容忍这种
 * 混合形态，但严格实现（自建 vLLM 等）会以 400 拒绝整个请求。
 * 规则与证据：docs/openai-responses-input-item-protocol.md。
 */
export function createOpenAIResponsesInputCompatFetch(baseFetch: ProviderFetch): ProviderFetch {
  return async (input, init) => baseFetch(input, applyOpenAIResponsesInputCompatibility(init));
}

function applyOpenAIResponsesInputCompatibility(
  init: RequestInit | undefined,
): RequestInit | undefined {
  if (typeof init?.body !== "string") return init;

  const body = safeParseRecord(init.body);
  if (!body) return init;

  const normalized = normalizeOpenAIResponsesInput(body);
  if (!normalized) return init;

  return { ...init, body: JSON.stringify(normalized) };
}

export function normalizeOpenAIResponsesInput(
  body: Record<string, unknown>,
): Record<string, unknown> | undefined {
  const input = Array.isArray(body.input) ? body.input : undefined;
  if (!input) return undefined;

  let changed = false;
  const normalizedInput = input.map((value) => {
    const item = asRecord(value);
    if (!item || typeof item.role !== "string" || !Array.isArray(item.content)) return value;
    // status 是 ResponseOutputMessage item 的必需字段，其中的 output_text 合法；easy message 一律按 input_text 归一化。
    if (typeof item.status === "string") return value;

    const text = singlePlainOutputTextPart(item.content);
    if (text !== undefined) {
      changed = true;
      return { ...item, content: text };
    }

    let contentChanged = false;
    const content = item.content.map((part) => {
      const record = asRecord(part);
      if (record?.type !== "output_text") return part;
      contentChanged = true;
      return { ...record, type: "input_text" };
    });
    if (!contentChanged) return value;

    changed = true;
    return { ...item, content };
  });

  return changed ? { ...body, input: normalizedInput } : undefined;
}

function singlePlainOutputTextPart(content: unknown[]): string | undefined {
  if (content.length !== 1) return undefined;
  const part = asRecord(content[0]);
  if (part?.type !== "output_text" || typeof part.text !== "string") return undefined;
  if (Object.keys(part).some((key) => key !== "type" && key !== "text")) return undefined;
  return part.text;
}

function safeParseRecord(value: string): Record<string, unknown> | undefined {
  try {
    return asRecord(JSON.parse(value));
  } catch {
    return undefined;
  }
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  return value as Record<string, unknown>;
}
