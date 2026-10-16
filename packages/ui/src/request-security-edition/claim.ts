import type { RequestVerificationProof } from "@escode/shared";
export function useRequestVerificationPrewarm(_visible: boolean): void {}
export async function prepareRequestVerificationClaim(
  signal: AbortSignal,
  _onVerifying: () => void,
): Promise<RequestVerificationProof | undefined> {
  signal.throwIfAborted();
  return {};
}
