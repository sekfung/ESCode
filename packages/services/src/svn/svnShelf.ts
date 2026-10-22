import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import type {
  SvnDeleteShelfRequest,
  SvnListShelvesResult,
  SvnRequest,
  SvnResult,
  SvnShelf,
  SvnShelveRequest,
  SvnShelveResult,
  SvnUnshelveRequest,
} from "@escode/shared";
import {
  DEFAULT_SVN_COMMAND_TIMEOUT_MS,
  DEFAULT_SVN_DIFF_TIMEOUT_MS,
  normalizeSvnPath,
} from "./config.js";
import { svnFailure } from "./repo/svnErrors.js";
import { buildUnversionedPatch, isSafeRelativePath, localTarget } from "./svnPaths.js";
import { readSvnChanges, readSvnWorkspaceInfo } from "./svnReads.js";
import type { SvnRuntime } from "./svnRuntime.js";

const SHELF_NAME_PATTERN = /^[A-Za-z0-9._-]{1,64}$/;

interface ShelfMetadata {
  version: 1;
  name: string;
  message: string;
  createdAt: number;
  paths: string[];
}

function resolveShelfName(name?: string): string | null {
  const trimmed = name?.trim();
  if (!trimmed) {
    return `shelf-${Date.now()}`;
  }
  return SHELF_NAME_PATTERN.test(trimmed) && trimmed !== "." && trimmed !== ".." ? trimmed : null;
}

function shelfDirectory(wcRoot: string): string {
  return join(wcRoot, ".svn", "escode-shelves");
}

function patchPath(wcRoot: string, name: string): string {
  return join(shelfDirectory(wcRoot), `${name}.patch`);
}

function metadataPath(wcRoot: string, name: string): string {
  return join(shelfDirectory(wcRoot), `${name}.json`);
}

async function resolveWcRoot(
  runtime: SvnRuntime,
  params: SvnRequest,
): Promise<{ ok: true; wcRoot: string } | { ok: false; failure: ReturnType<typeof svnFailure> }> {
  const workspace = await readSvnWorkspaceInfo(runtime, params);
  if (!workspace.ok) {
    return { ok: false, failure: workspace };
  }
  return { ok: true, wcRoot: workspace.info.wcRoot ?? params.workspacePath };
}

function parseMetadata(name: string, raw: string): SvnShelf | null {
  try {
    const parsed = JSON.parse(raw) as Partial<ShelfMetadata>;
    if (
      parsed.name !== name ||
      typeof parsed.message !== "string" ||
      typeof parsed.createdAt !== "number" ||
      !Array.isArray(parsed.paths) ||
      !parsed.paths.every((path) => typeof path === "string")
    ) {
      return null;
    }
    return {
      name,
      message: parsed.message,
      createdAt: parsed.createdAt,
      paths: parsed.paths,
    };
  } catch {
    return null;
  }
}

export async function listSvnShelves(
  runtime: SvnRuntime,
  params: SvnRequest,
): Promise<SvnResult<SvnListShelvesResult>> {
  const root = await resolveWcRoot(runtime, params);
  if (!root.ok) {
    return root.failure;
  }
  let files: string[];
  try {
    files = await readdir(shelfDirectory(root.wcRoot));
  } catch {
    return { ok: true, shelves: [] };
  }
  const shelves: SvnShelf[] = [];
  for (const file of files) {
    if (!file.endsWith(".json")) {
      continue;
    }
    const name = file.slice(0, -".json".length);
    try {
      const shelf = parseMetadata(
        name,
        await readFile(join(shelfDirectory(root.wcRoot), file), "utf-8"),
      );
      if (shelf) {
        shelves.push(shelf);
      }
    } catch {
      // 忽略损坏或并发删除的 shelf
    }
  }
  shelves.sort((left, right) => right.createdAt - left.createdAt);
  return { ok: true, shelves };
}

export async function shelveSvnChanges(
  runtime: SvnRuntime,
  params: SvnShelveRequest,
): Promise<SvnResult<SvnShelveResult>> {
  const paths = [...new Set(params.paths.map(normalizeSvnPath))];
  if (paths.length === 0 || !paths.every(isSafeRelativePath)) {
    return svnFailure("invalid-request", "paths must be relative to the workspace");
  }
  const name = resolveShelfName(params.name);
  if (!name) {
    return svnFailure("invalid-request", "invalid shelf name");
  }
  const changes = await readSvnChanges(runtime, params);
  if (!changes.ok) {
    return changes;
  }
  const byPath = new Map(changes.changes.map((change) => [change.path, change]));
  const selected = paths.map((path) => byPath.get(path));
  if (selected.some((change) => !change)) {
    return svnFailure("invalid-request", "selected paths have no changes to shelve");
  }
  for (const change of selected) {
    if (!change) {
      continue;
    }
    if (change.isBinary) {
      return svnFailure("invalid-request", "binary files cannot be shelved");
    }
    if (change.isDirectory && change.status === "unversioned") {
      return svnFailure("invalid-request", "unversioned directories cannot be shelved");
    }
  }
  const root = await resolveWcRoot(runtime, params);
  if (!root.ok) {
    return root.failure;
  }
  const directory = shelfDirectory(root.wcRoot);
  const patchFile = patchPath(root.wcRoot, name);
  const metadataFile = metadataPath(root.wcRoot, name);
  try {
    await readFile(patchFile);
    return svnFailure("invalid-request", "a shelf with this name already exists");
  } catch {
    // 不存在，继续
  }

  return await runtime.enqueueWrite(params, async () => {
    // `svn diff` / `revert` 不接受未纳入版本控制的路径；它们由合成 patch 处理。
    const versionedPaths = paths.filter((path) => byPath.get(path)?.status !== "unversioned");
    const missing = selected
      .filter((change) => change?.status === "missing")
      .map((change) => change!.path);
    if (missing.length > 0) {
      const deleted = await runtime.run({
        cwd: params.workspacePath,
        args: ["delete", "--force", "--", ...missing.map(localTarget)],
      });
      if (!deleted.ok) {
        return deleted.failure;
      }
    }
    const diff = await runtime.run({
      cwd: params.workspacePath,
      args: [
        "diff",
        "--internal-diff",
        "--depth",
        "infinity",
        "--",
        ...versionedPaths.map(localTarget),
      ],
      timeoutMs: DEFAULT_SVN_DIFF_TIMEOUT_MS,
    });
    if (!diff.ok) {
      if (missing.length > 0) {
        await runtime.run({
          cwd: params.workspacePath,
          args: ["revert", "--depth", "infinity", "--", ...missing.map(localTarget)],
        });
      }
      return diff.failure;
    }
    const patches: string[] = [];
    if (diff.result.stdout.trim()) {
      patches.push(diff.result.stdout.trimEnd());
    }
    for (const change of selected) {
      if (change?.status !== "unversioned") {
        continue;
      }
      const absolute = resolve(params.workspacePath, change.path);
      const synthetic = await buildUnversionedPatch(absolute, change.path);
      if (synthetic.isBinary) {
        return svnFailure("invalid-request", "binary files cannot be shelved");
      }
      if (!synthetic.patch || synthetic.truncated) {
        return svnFailure("invalid-request", "unversioned file is too large to shelve");
      }
      patches.push(synthetic.patch);
    }
    const patch = patches.join("\n");
    if (!patch.trim()) {
      return svnFailure("invalid-request", "there are no changes to shelve");
    }
    const metadata: ShelfMetadata = {
      version: 1,
      name,
      message: params.message?.trim() ?? "",
      createdAt: Date.now(),
      paths,
    };
    await mkdir(directory, { recursive: true });
    await writeFile(patchFile, patch, "utf-8");
    await writeFile(metadataFile, JSON.stringify(metadata, null, 2), "utf-8");

    const reverted = await runtime.run({
      cwd: params.workspacePath,
      args: ["revert", "--depth", "infinity", "--", ...versionedPaths.map(localTarget)],
      timeoutMs: DEFAULT_SVN_COMMAND_TIMEOUT_MS,
    });
    if (!reverted.ok) {
      await rm(patchFile, { force: true }).catch(() => undefined);
      await rm(metadataFile, { force: true }).catch(() => undefined);
      return reverted.failure;
    }
    for (const change of selected) {
      if (change && (change.status === "added" || change.status === "unversioned")) {
        await rm(resolve(params.workspacePath, change.path), {
          recursive: true,
          force: true,
        }).catch(() => undefined);
      }
    }
    const shelf: SvnShelf = {
      name,
      message: metadata.message,
      createdAt: metadata.createdAt,
      paths,
    };
    return { ok: true as const, shelf };
  });
}

export async function unshelveSvnChanges(
  runtime: SvnRuntime,
  params: SvnUnshelveRequest,
): Promise<SvnResult> {
  const name = resolveShelfName(params.name);
  if (!name) {
    return svnFailure("invalid-request", "invalid shelf name");
  }
  const root = await resolveWcRoot(runtime, params);
  if (!root.ok) {
    return root.failure;
  }
  const patchFile = patchPath(root.wcRoot, name);
  try {
    await readFile(patchFile);
  } catch {
    return svnFailure("shelf-not-found", "shelf was not found");
  }
  return await runtime.enqueueWrite(params, async () => {
    const outcome = await runtime.run({
      cwd: params.workspacePath,
      args: ["patch", "--", patchFile],
      timeoutMs: DEFAULT_SVN_DIFF_TIMEOUT_MS,
    });
    return outcome.ok ? { ok: true } : outcome.failure;
  });
}

export async function deleteSvnShelf(
  runtime: SvnRuntime,
  params: SvnDeleteShelfRequest,
): Promise<SvnResult> {
  const name = resolveShelfName(params.name);
  if (!name) {
    return svnFailure("invalid-request", "invalid shelf name");
  }
  const root = await resolveWcRoot(runtime, params);
  if (!root.ok) {
    return root.failure;
  }
  return await runtime.enqueueWrite(params, async () => {
    let found = false;
    for (const file of [patchPath(root.wcRoot, name), metadataPath(root.wcRoot, name)]) {
      try {
        await readFile(file);
        found = true;
      } catch {
        // 尝试下一个文件
      }
      await rm(file, { force: true }).catch(() => undefined);
    }
    return found ? { ok: true } : svnFailure("shelf-not-found", "shelf was not found");
  });
}
