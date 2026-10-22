import {
  SVN_MIN_SUPPORTED_VERSION,
  getWorkspaceVcsKey,
  type SvnAvailability,
  type SvnCredentials,
  type SvnFailure,
  type SvnRequest,
} from "@escode/shared";
import type { ServiceLogger } from "#src/logger/serviceLogger.js";
import type { ISettingService } from "../setting/setting.js";
import { compareSvnVersions } from "./config.js";
import {
  createSvnCommandProvider,
  type SvnCommandExecutionResult,
  type SvnCommandProvider,
} from "./providers/svnCommandProvider.js";
import { classifySvnError, summarizeSvnStderr, svnFailure } from "./repo/svnErrors.js";

export interface ResolvedSvnBinary {
  binaryPath: string;
  version: string;
  supportsPasswordFromStdin: boolean;
}

export interface SvnRunOptions {
  cwd: string;
  args: string[];
  credentials?: SvnCredentials;
  timeoutMs?: number;
  maxOutputBytes?: number;
}

export type SvnRunOutcome =
  | { ok: true; result: SvnCommandExecutionResult }
  | { ok: false; failure: SvnFailure; result?: SvnCommandExecutionResult };

export interface SvnRuntime {
  logger?: ServiceLogger;
  resolveBinary(): Promise<
    { ok: true; binary: ResolvedSvnBinary } | { ok: false; availability: SvnAvailability }
  >;
  run(options: SvnRunOptions): Promise<SvnRunOutcome>;
  /** 同一 workspace 身份 key 的写操作串行执行；读操作不进入队列。 */
  enqueueWrite<T>(request: SvnRequest, job: () => Promise<T>): Promise<T>;
}

export function createSvnRuntime(options: {
  commandProvider?: SvnCommandProvider;
  settingService?: Pick<ISettingService, "get">;
  logger?: ServiceLogger;
}): SvnRuntime {
  const commandProvider = options.commandProvider ?? createSvnCommandProvider();
  const logger = options.logger;
  const writeQueues = new Map<string, Promise<unknown>>();

  const runtime: SvnRuntime = {
    logger,

    async resolveBinary() {
      const settings = await options.settingService?.get().catch(() => null);
      const resolution = await commandProvider.resolveSvnBinary(settings?.svnBinaryPath);
      if (!resolution) {
        return { ok: false, availability: { status: "not-found" } };
      }
      if (compareSvnVersions(resolution.version, SVN_MIN_SUPPORTED_VERSION) < 0) {
        return {
          ok: false,
          availability: {
            status: "unsupported-version",
            version: resolution.version,
            binaryPath: resolution.binaryPath,
          },
        };
      }
      return {
        ok: true,
        binary: {
          ...resolution,
          supportsPasswordFromStdin: compareSvnVersions(resolution.version, "1.10.0") >= 0,
        },
      };
    },

    async run(runOptions) {
      const binary = await runtime.resolveBinary();
      if (!binary.ok) {
        return {
          ok: false,
          failure: svnFailure(
            "svn-unavailable",
            binary.availability.status === "unsupported-version"
              ? `svn ${binary.availability.version} is older than ${SVN_MIN_SUPPORTED_VERSION}`
              : "svn executable was not found",
          ),
        };
      }

      const args = ["--non-interactive", ...runOptions.args];
      let stdin: string | undefined;
      if (runOptions.credentials) {
        const { username, password, storeInSvnCache } = runOptions.credentials;
        args.unshift("--username", username);
        // svn ≥ 1.10 从 stdin 读密码，避免密码出现在进程列表里。
        if (binary.binary.supportsPasswordFromStdin) {
          args.unshift("--password-from-stdin");
          stdin = password;
        } else {
          args.unshift("--password", password);
        }
        if (!storeInSvnCache) {
          args.unshift("--no-auth-cache");
        }
      }

      const result = await commandProvider.run({
        binaryPath: binary.binary.binaryPath,
        cwd: runOptions.cwd,
        args,
        stdin,
        timeoutMs: runOptions.timeoutMs,
        maxOutputBytes: runOptions.maxOutputBytes,
      });

      // 日志只记录子命令名，不记录参数：参数可能包含用户名、密码或提交路径。
      logger?.debug(undefined, "svn 命令完成", {
        subcommand: runOptions.args[0],
        exitCode: result.exitCode,
        durationMs: result.durationMs,
        timedOut: result.timedOut,
      });

      if (result.timedOut) {
        return { ok: false, failure: svnFailure("timeout", "svn command timed out"), result };
      }
      if (result.exitCode !== 0 && !result.outputTruncated) {
        const issue = classifySvnError(result.stderr);
        if (
          issue === "working-copy-locked" ||
          issue === "auth-required" ||
          issue === "out-of-date"
        ) {
          logger?.warn(undefined, "svn 命令失败", { subcommand: runOptions.args[0], issue });
        }
        return {
          ok: false,
          failure: svnFailure(issue, summarizeSvnStderr(result.stderr) || "svn command failed"),
          result,
        };
      }
      return { ok: true, result };
    },

    enqueueWrite(request, job) {
      const key = getWorkspaceVcsKey(request);
      const previous = writeQueues.get(key) ?? Promise.resolve();
      const current = previous.then(job, job);
      const tail = current.catch(() => undefined);
      writeQueues.set(key, tail);
      void tail.then(() => {
        if (writeQueues.get(key) === tail) {
          writeQueues.delete(key);
        }
      });
      return current;
    },
  };

  return runtime;
}
