const ESCODE_PROCESS_PREFIX = "escode";
const MAX_PROCESS_NAME_SEGMENT_LENGTH = 24;

function sanitizeProcessNameSegment(value: string | null | undefined): string | null {
  if (!value) {
    return null;
  }

  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (!normalized) {
    return null;
  }

  return normalized.slice(0, MAX_PROCESS_NAME_SEGMENT_LENGTH);
}

function joinESCodeProcessName(...segments: Array<string | null | undefined>): string {
  const sanitizedSegments = segments
    .map((segment) => sanitizeProcessNameSegment(segment))
    .filter((segment): segment is string => Boolean(segment));
  return [ESCODE_PROCESS_PREFIX, ...sanitizedSegments].join("-");
}

function pickWorkspaceTag(workspacePath: string | null | undefined): string | undefined {
  const trimmedPath = workspacePath?.trim();
  if (!trimmedPath) {
    return undefined;
  }

  const parts = trimmedPath.split(/[\\/]+/).filter(Boolean);
  return parts.at(-1) ?? trimmedPath;
}

export function formatESCodeMainProcessName(): string {
  return joinESCodeProcessName("main");
}

export function formatESCodeGpuProcessName(): string {
  return joinESCodeProcessName("gpu");
}

export function formatESCodeHostProcessName(label?: string): string {
  return joinESCodeProcessName("host", label);
}

export function formatESCodeRendererProcessName(windowTitle?: string): string {
  const normalizedTitle = windowTitle?.trim();
  if (!normalizedTitle || normalizedTitle === "ESCode") {
    return joinESCodeProcessName("renderer", "main");
  }

  if (normalizedTitle === "Resource Manager") {
    return joinESCodeProcessName("renderer", "resource-manager");
  }

  const remoteWindowPrefix = "ESCode - ";
  if (normalizedTitle.startsWith(remoteWindowPrefix)) {
    return joinESCodeProcessName(
      "renderer",
      "remote",
      normalizedTitle.slice(remoteWindowPrefix.length),
    );
  }

  return joinESCodeProcessName("renderer", normalizedTitle);
}

export function formatESCodeAgentProcessName(provider: string, workspacePath?: string): string {
  return joinESCodeProcessName("agent", provider, pickWorkspaceTag(workspacePath));
}

export function formatESCodeUtilityProcessName(name?: string, type = "utility"): string {
  return joinESCodeProcessName(type, name);
}
