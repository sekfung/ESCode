import { randomUUID } from "node:crypto";
import { Client as SSHClient } from "ssh2";

export interface RemoteSSHCommandConfig {
  host: string;
  port: number;
  username: string;
  password: string;
}

/**
 * 在 E2E 远端目标执行 POSIX 命令；只用于夹具准备，不参与被测代码路径。
 */
export async function runRemoteCommand(
  caseMarker: string,
  config: RemoteSSHCommandConfig,
  command: string,
): Promise<string> {
  return new Promise((resolve, reject) => {
    const client = new SSHClient();
    client.once("ready", () => {
      client.exec(command, (error, stream) => {
        if (error) {
          client.end();
          reject(error);
          return;
        }
        let stdout = "";
        let stderr = "";
        stream.on("data", (chunk: Buffer) => {
          stdout += chunk.toString("utf8");
        });
        stream.stderr.on("data", (chunk: Buffer) => {
          stderr += chunk.toString("utf8");
        });
        stream.once("close", (code: number | null) => {
          client.end();
          if (code === 0) resolve(stdout);
          else reject(new Error(`${caseMarker}: remote command failed code=${code} ${stderr}`));
        });
      });
    });
    client.once("error", reject);
    client.connect({
      host: config.host,
      port: config.port,
      username: config.username,
      password: config.password,
      readyTimeout: 30_000,
    });
  });
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

const INSTALL_MARKER = '"$HOME/.escode/server/install.json"';

/**
 * 读取远端持久 `sourceServerId`；不存在时按 R2 ownership marker 契约原子创建。
 *
 * Rust Host 的 identity probe 以该 marker 为唯一身份来源（spec §11.12），因此 E2E 预置
 * Profile 时必须使用同一 identity，不能再用随机 UUID 伪造绑定。
 */
export async function readOrSeedRemoteSourceServerId(
  caseMarker: string,
  config: RemoteSSHCommandConfig,
): Promise<string> {
  const canonicalRoot = (
    await runRemoteCommand(
      caseMarker,
      config,
      'mkdir -p "$HOME/.escode/server" && cd "$HOME/.escode/server" && pwd -P',
    )
  ).trim();
  if (!canonicalRoot.startsWith("/")) {
    throw new Error(`${caseMarker}: 远端 server root 不是绝对路径: ${canonicalRoot}`);
  }
  const existing = (
    await runRemoteCommand(
      caseMarker,
      config,
      `if [ -f ${INSTALL_MARKER} ]; then cat ${INSTALL_MARKER}; fi`,
    )
  ).trim();
  if (existing) return parseInstallationId(caseMarker, existing);

  const installationId = randomUUID();
  const payload = JSON.stringify({
    product: "escode-server",
    schemaVersion: 1,
    canonicalServerRoot: canonicalRoot,
    installationId,
    installedAt: Date.now(),
  });
  const created = await runRemoteCommand(
    caseMarker,
    config,
    [
      'mkdir -p "$HOME/.escode/server/run"',
      `tmp="$HOME/.escode/server/run/.e2e-install-$$-\${RANDOM:-0}.tmp"`,
      `printf '%s\\n' ${shellQuote(payload)} > "$tmp"`,
      `ln "$tmp" ${INSTALL_MARKER} 2>/dev/null || true`,
      'rm -f "$tmp"',
      `cat ${INSTALL_MARKER}`,
    ].join(" && "),
  );
  return parseInstallationId(caseMarker, created.trim());
}

function parseInstallationId(caseMarker: string, content: string): string {
  let marker: unknown;
  try {
    marker = JSON.parse(content);
  } catch (error) {
    throw new Error(`${caseMarker}: 远端 install.json 不是合法 JSON: ${String(error)}`);
  }
  const installationId =
    marker && typeof marker === "object"
      ? (marker as { installationId?: unknown }).installationId
      : undefined;
  if (typeof installationId !== "string" || !/^[0-9a-f-]{36}$/i.test(installationId)) {
    throw new Error(`${caseMarker}: 远端 install.json 缺少合法 installationId`);
  }
  return installationId;
}
