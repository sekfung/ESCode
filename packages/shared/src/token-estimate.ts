import { ESTIMATED_TOKEN_CHAR_DIVISOR } from "./usage-stats.js";

/**
 * 估算文本的 token 数量。
 *
 * CLI 上下文预算与 Renderer 的生成速度读数共用这一份口径：流式阶段协议不携带逐 chunk token 数
 * （provider 只在请求结束时给 usage），两侧都只能按流出文本估算，必须同源，否则同一个会话会出现
 * 两个互相矛盾的估算。
 */
export function estimateTokens(text: string): number {
  // 中文字符通常比英文字符占用更多 token，因此按两个估算字符计入。
  const chineseChars = (text.match(/[一-鿿]/g) || []).length;
  const otherChars = text.length - chineseChars;

  return Math.ceil((chineseChars * 2 + otherChars) / ESTIMATED_TOKEN_CHAR_DIVISOR);
}
