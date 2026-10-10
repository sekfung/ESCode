import { SerialError, type SerialErrorCode } from "./serial.js";

/**
 * 把 bindings-cpp / OS 的打开失败消息映射为稳定错误码。
 * Windows 的独占冲突表现为 Access denied，类 Unix 的独占锁冲突表现为 EBUSY / Cannot lock port；
 * 类 Unix 的 Permission denied 才是权限问题（未加入 dialout/uucp 组）。
 */
const OPEN_ERROR_PATTERNS: Array<[RegExp, SerialErrorCode]> = [
  [/access denied|resource busy|cannot lock port|port is locked|ebusy/i, "busy"],
  [/permission denied|eacces/i, "denied"],
  [/file not found|no such file|does not exist|enoent/i, "notFound"],
  [/invalid argument|einval/i, "invalidConfig"],
];

export function mapSerialOpenError(error: unknown): SerialError {
  if (error instanceof SerialError) return error;
  const message = error instanceof Error ? error.message : String(error);
  const matched = OPEN_ERROR_PATTERNS.find(([pattern]) => pattern.test(message));
  return new SerialError(matched?.[1] ?? "io", message);
}
