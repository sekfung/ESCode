/** Renderer 本地单调时钟的 epoch 毫秒读数；只用于本机体感指标。 */
export function localMonotonicNow(): number {
  return performance.timeOrigin + performance.now();
}
