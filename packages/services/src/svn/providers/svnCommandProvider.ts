import { spawn, type ChildProcess } from "node:child_process";
import { DEFAULT_SVN_COMMAND_TIMEOUT_MS, DEFAULT_SVN_OUTPUT_BYTES } from "../config.js";
import {
  createSvnEnvironmentProvider,
  type SvnBinaryResolution,
  type SvnEnvironmentProvider,
} from "./svnEnvironmentProvider.js";

export interface SvnCommandExecutionOptions {
  binaryPath: string;
  cwd: string;
  args: string[];
  /** 写入子进程 stdin 后立即关闭；用于 --password-from-stdin。 */
  stdin?: string;
  timeoutMs?: number;
  maxOutputBytes?: number;
}

export interface SvnCommandExecutionResult {
  stdout: string;
  stderr: string;
  exitCode: number | null;
  durationMs: number;
  timedOut: boolean;
  outputTruncated: boolean;
}

export interface SvnCommandProvider {
  resolveSvnBinary(configuredPath?: string): Promise<SvnBinaryResolution | null>;
  run(options: SvnCommandExecutionOptions): Promise<SvnCommandExecutionResult>;
}

const TIMEOUT_FORCE_KILL_GRACE_MS = 2_000;

function forceKill(child: ChildProcess): void {
  if (process.platform === "win32" && typeof child.pid === "number") {
    spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true,
    }).once("error", () => {});
    return;
  }
  child.kill("SIGKILL");
}

export function createSvnCommandProvider(options?: {
  environmentProvider?: SvnEnvironmentProvider;
}): SvnCommandProvider {
  const environmentProvider = options?.environmentProvider ?? createSvnEnvironmentProvider();

  return {
    async resolveSvnBinary(configuredPath?: string) {
      return await environmentProvider.resolveSvnBinary(configuredPath);
    },

    async run(command: SvnCommandExecutionOptions): Promise<SvnCommandExecutionResult> {
      const timeoutMs = command.timeoutMs ?? DEFAULT_SVN_COMMAND_TIMEOUT_MS;
      const maxOutputBytes = command.maxOutputBytes ?? DEFAULT_SVN_OUTPUT_BYTES;
      const startedAt = Date.now();

      return await new Promise<SvnCommandExecutionResult>((resolve) => {
        const stdoutChunks: Buffer[] = [];
        const stderrChunks: Buffer[] = [];
        let outputBytes = 0;
        let outputTruncated = false;
        let timedOut = false;
        let settled = false;
        let forceKillTimer: NodeJS.Timeout | undefined;

        const child = spawn(command.binaryPath, command.args, {
          cwd: command.cwd,
          env: environmentProvider.createCommandEnv(),
          stdio: [command.stdin === undefined ? "ignore" : "pipe", "pipe", "pipe"],
          windowsHide: true,
        });

        const settle = (exitCode: number | null, extraStderr?: string) => {
          if (settled) {
            return;
          }
          settled = true;
          clearTimeout(timer);
          clearTimeout(forceKillTimer);
          // 按字节收集后统一解码，避免多字节 UTF-8 字符被 chunk 边界截断成乱码。
          const stderr = Buffer.concat(stderrChunks).toString("utf-8");
          resolve({
            stdout: Buffer.concat(stdoutChunks).toString("utf-8"),
            stderr: extraStderr ? `${stderr}${extraStderr}` : stderr,
            exitCode,
            durationMs: Date.now() - startedAt,
            timedOut,
            outputTruncated,
          });
        };

        const timer = setTimeout(() => {
          timedOut = true;
          child.kill();
          forceKillTimer = setTimeout(() => {
            forceKill(child);
            // 进程或其 ssh 隧道仍拖住管道时直接结束，避免写队列被永久占住。
            settle(null);
          }, TIMEOUT_FORCE_KILL_GRACE_MS);
        }, timeoutMs);

        const append = (target: Buffer[], chunk: Buffer) => {
          if (outputTruncated) {
            return;
          }
          if (outputBytes + chunk.byteLength > maxOutputBytes) {
            outputTruncated = true;
            child.kill();
            return;
          }
          outputBytes += chunk.byteLength;
          target.push(chunk);
        };

        child.stdout?.on("data", (chunk: Buffer) => append(stdoutChunks, chunk));
        child.stderr?.on("data", (chunk: Buffer) => append(stderrChunks, chunk));
        child.once("error", (error) => {
          // spawn 失败（二进制被删除、cwd 不存在）统一降级为失败结果，由上层归类，
          // 不向 RPC 调用方抛出进程级异常。
          settle(-2, error instanceof Error ? error.message : String(error));
        });
        child.once("close", (exitCode) => settle(exitCode));

        if (command.stdin !== undefined && child.stdin) {
          child.stdin.once("error", () => {});
          child.stdin.end(command.stdin);
        }
      });
    },
  };
}
