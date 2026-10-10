import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ESCodeStdioTapDevState } from "@escode/shared";
import { getAppConfigDir } from "#src/paths.js";
import { isEffectiveDevelopmentNodeEnv } from "#src/runtime-tools/nodeEnv.js";

interface ESCodeStdioTapStateFile {
  enabled?: boolean;
}

function isESCodeStdioTapDevVisible(): boolean {
  return isEffectiveDevelopmentNodeEnv();
}

function getESCodeStdioTapDevDir(): string {
  return join(getAppConfigDir(), "dev");
}

export function getESCodeStdioTapDevLogDir(): string {
  return join(getESCodeStdioTapDevDir(), "stdio-traffic");
}

function getESCodeStdioTapDevStatePath(): string {
  return join(getESCodeStdioTapDevDir(), "escode-stdio-tap.json");
}

function readStateFile(path: string): ESCodeStdioTapStateFile {
  if (!existsSync(path)) {
    return {};
  }

  try {
    const parsed = JSON.parse(readFileSync(path, "utf-8")) as unknown;
    return parsed && typeof parsed === "object" ? (parsed as ESCodeStdioTapStateFile) : {};
  } catch {
    return {};
  }
}

export function readESCodeStdioTapDevState(): ESCodeStdioTapDevState {
  const visible = isESCodeStdioTapDevVisible();
  const statePath = getESCodeStdioTapDevStatePath();
  const fileState = readStateFile(statePath);
  return {
    enabled: visible && fileState.enabled === true,
    visible,
    logDir: getESCodeStdioTapDevLogDir(),
    statePath,
  };
}

export function setESCodeStdioTapDevEnabled(enabled: boolean): ESCodeStdioTapDevState {
  const visible = isESCodeStdioTapDevVisible();
  const statePath = getESCodeStdioTapDevStatePath();
  mkdirSync(getESCodeStdioTapDevDir(), { recursive: true });
  writeFileSync(
    statePath,
    `${JSON.stringify(
      {
        // 开发态 stdio 抓包是高频原始协议帧，只能通过显式开关写旁路文件，避免误进生产日志。
        enabled: visible && enabled,
        updatedAt: new Date().toISOString(),
      },
      null,
      2,
    )}\n`,
  );
  return readESCodeStdioTapDevState();
}
