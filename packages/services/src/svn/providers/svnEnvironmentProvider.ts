import { spawn } from "node:child_process";
import {
  DEFAULT_SVN_DISCOVERY_TIMEOUT_MS,
  getSvnBinaryCandidates,
  getSvnCommandEnv,
} from "../config.js";

export interface SvnBinaryResolution {
  binaryPath: string;
  version: string;
}

export interface SvnEnvironmentProvider {
  /** configuredPath 来自 Host 设置；变化时重新探测。 */
  resolveSvnBinary(configuredPath?: string): Promise<SvnBinaryResolution | null>;
  createCommandEnv(): NodeJS.ProcessEnv;
}

async function readSvnVersion(candidate: string, env: NodeJS.ProcessEnv): Promise<string | null> {
  return await new Promise<string | null>((resolve) => {
    let stdout = "";
    const child = spawn(candidate, ["--version", "--quiet"], {
      env,
      stdio: ["ignore", "pipe", "ignore"],
      windowsHide: true,
    });

    const timer = setTimeout(() => {
      child.kill();
      resolve(null);
    }, DEFAULT_SVN_DISCOVERY_TIMEOUT_MS);

    child.stdout?.on("data", (chunk: Buffer) => {
      stdout += chunk.toString("utf-8");
    });
    child.once("error", () => {
      clearTimeout(timer);
      resolve(null);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      const version = stdout.trim().match(/^\d+\.\d+(\.\d+)?/)?.[0];
      resolve(code === 0 && version ? version : null);
    });
  });
}

export function createSvnEnvironmentProvider(): SvnEnvironmentProvider {
  // 二进制探测结果按手动路径缓存：设置不变时一次会话只探测一次，设置变化后自然失效。
  const cache = new Map<string, Promise<SvnBinaryResolution | null>>();

  return {
    async resolveSvnBinary(configuredPath?: string): Promise<SvnBinaryResolution | null> {
      const key = configuredPath?.trim() ?? "";
      const cached = cache.get(key);
      if (cached) {
        return await cached;
      }
      const env = getSvnCommandEnv();
      const pending = (async () => {
        for (const candidate of getSvnBinaryCandidates(key)) {
          const version = await readSvnVersion(candidate, env);
          if (version) {
            return { binaryPath: candidate, version };
          }
        }
        return null;
      })();
      cache.set(key, pending);
      const result = await pending;
      if (!result) {
        // 未找到时不缓存，用户装好 svn 后刷新即可识别，无需重启 Host。
        cache.delete(key);
      }
      return result;
    },

    createCommandEnv(): NodeJS.ProcessEnv {
      return getSvnCommandEnv();
    },
  };
}
