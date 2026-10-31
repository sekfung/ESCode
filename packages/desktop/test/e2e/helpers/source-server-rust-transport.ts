import { readdirSync, readFileSync } from "node:fs";
import { runSSHCommand, type SSHConnectionConfig } from "./ssh-remote-p0.js";

interface ProcEntry {
  pid: number;
  ppid: number;
  comm: string;
  cmdline: string;
}

function readProcTable(): ProcEntry[] {
  const entries: ProcEntry[] = [];
  for (const name of readdirSync("/proc")) {
    if (!/^\d+$/u.test(name)) continue;
    try {
      const stat = readFileSync(`/proc/${name}/stat`, "utf8");
      const commStart = stat.indexOf("(");
      const commEnd = stat.lastIndexOf(")");
      const fields = stat.slice(commEnd + 2).split(" ");
      const cmdline = readFileSync(`/proc/${name}/cmdline`, "utf8")
        .split("\0")
        .filter(Boolean)
        .join(" ");
      entries.push({
        pid: Number(name),
        ppid: Number(fields[1]),
        comm: stat.slice(commStart + 1, commEnd),
        cmdline,
      });
    } catch {
      // 进程可能在扫描期间退出；跳过即可。
    }
  }
  return entries;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * E2E 只杀本进程树里 Rust Host 的 SSH transport 子进程，模拟真实链路中断；
 * 远端 Source Server 是否存活由 transport 自身语义决定，测试不再旁路操作远端。
 */
export async function killRustHostTransport(
  caseMarker: string,
): Promise<{ mainPid: number; hostPid: number; sshPid: number }> {
  const mainPid = (await browser.electron.execute(() => process.pid)) as number;
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const table = readProcTable();
    const host = table.find((entry) => entry.ppid === mainPid && entry.comm === "escode-host");
    if (host) {
      const ssh = table.find((entry) => entry.ppid === host.pid && entry.comm === "ssh");
      if (ssh) {
        process.kill(ssh.pid, "SIGKILL");
        await browser.waitUntil(() => !readProcTable().some((entry) => entry.pid === ssh.pid), {
          timeout: 30_000,
          interval: 100,
          timeoutMsg: `${caseMarker}: SSH transport ${ssh.pid} 没有被终止`,
        });
        return { mainPid, hostPid: host.pid, sshPid: ssh.pid };
      }
    }
    await sleep(100);
  }
  throw new Error(`${caseMarker}: 没有找到 Rust Host ${mainPid} 的 SSH transport 子进程`);
}

const REMOVE_REMOTE_PROVIDER_SCRIPT = [
  'const fs=require("fs");',
  'const p="/root/.escode/v2/provider_config.json";',
  'const data=JSON.parse(fs.readFileSync(p,"utf8"));',
  'const inc=JSON.parse(Buffer.from(process.argv[1],"base64").toString("utf8"));',
  "const c=data.config;",
  "if(Array.isArray(c.providerConfigRules?.providerRules)){c.providerConfigRules.providerRules=c.providerConfigRules.providerRules.filter(r=>r&&r.providerId!==inc.providerId);}",
  "if(Array.isArray(c.modelConfigRules?.providerModelRules)){c.modelConfigRules.providerModelRules=c.modelConfigRules.providerModelRules.filter(r=>r&&!(r.providerId===inc.providerId&&r.modelId===inc.modelId));}",
  "fs.writeFileSync(p,JSON.stringify(data,null,2));",
].join("");

/** 用例结束清掉本轮写入的远端 replay Provider，避免污染共享 fixture 容器的模型清单。 */
export async function removeRemoteSourceServerProvider(
  config: SSHConnectionConfig,
  providerId: string,
  modelId: string,
): Promise<void> {
  const payload = Buffer.from(JSON.stringify({ providerId, modelId }), "utf8").toString("base64");
  const result = await runSSHCommand(
    config,
    `/root/.escode/server/node -e '${REMOVE_REMOTE_PROVIDER_SCRIPT}' ${payload}`,
  );
  if (result.code !== 0) {
    throw new Error(`清理远端 Provider 配置失败: ${result.stderr}`);
  }
}
