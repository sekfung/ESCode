import { logger } from "./logger.js";
import { initializeCrashCapture, type CrashCapturePaths } from "./desktopCrashCapture.js";

// 须在 desktopEarlyDataBaseDirBootstrap 注入 dataBaseDir 之后配置 crashDumps。
// 没有远端 crash 上报通道时启动本地 crashReporter，并保留本地 dump 归档。
export const crashCapturePaths: CrashCapturePaths = initializeCrashCapture(logger, false);
