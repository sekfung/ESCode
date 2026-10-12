#!/usr/bin/env node
/**
 * 构建期从 CDN 下载「构建目标架构对应的 Linux 远程 release」，落到 bundled-remote-assets，
 * 供 electron-builder 复制进 resources/remote-assets。
 *
 * 为什么是「构建期下载」而不是「先本地构建再打包」：
 * CDN 上已经有发布流水线产出的同一份制品（manifest + 组件归档），安装包只要把它们取下来即可；
 * 重新跑一遍跨平台 remote 资产的构建既慢又容易和发布制品不一致。
 * 下载物只存在于构建工作区（bundled-remote-assets 已 gitignore），不需要入库。
 *
 * 平台映射（见 docs/specs/offline-remote-assets.md）：远程后端只支持 POSIX，且 Windows 桌面的
 * 远程目标（WSL / SSH / Docker）都是 Linux，所以按构建目标的架构取同架构的 linux release：
 * x64 -> linux-x64，arm64 -> linux-arm64。
 */
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { extractTarGzArchive } from "@escode/server/remote/localTarGz.js";
import {
  buildComponentArtifactUrlCandidates,
  buildReleaseAssetUrlCandidates,
  buildReleaseBaseCandidates,
  normalizeRemoteAssetRelativePath,
} from "@escode/server/remote/remoteAssetCdn.js";
import { resolveRemoteCdnReleaseBaseCandidates } from "../../../scripts/remote-cdn-roots.mjs";
import { getBuildMetadata } from "./build-metadata.mjs";
import { getTargetPlatform } from "./target-platform.mjs";

interface RemoteAssetManifestComponent {
  id: string;
  version: string;
  sha256: string;
  artifactPath: string;
  mount: string;
}

interface RemoteAssetManifest {
  schemaVersion: number;
  appVersion: string;
  platformArch: string;
  components: RemoteAssetManifestComponent[];
}

const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopDir = resolve(scriptDir, "..");
const bundledDir = join(desktopDir, "bundled-remote-assets");

// 目录名必须与运行时 ESCODE_VERSION 逐字一致：主进程按
// <resources>/remote-assets/releases/<ESCODE_VERSION> 判断内置资源是否可用，名字对不上会静默退回 CDN。
const stagedVersion = getBuildMetadata().appVersion;
const remotePlatform = `linux-${getTargetPlatform().arch}`;
const releaseDir = join(bundledDir, "releases", stagedVersion);

/**
 * 必须存在的条目：与运行时 findMissingMockReleasePaths 的组件必需路径保持一致。
 * 构建期先失败，避免安装包带着不完整的本地源出厂，让用户连接时才退化成 CDN 下载。
 */
function resolveRequiredEntries(): string[] {
  return [
    join("server", "zcode-server.cjs"),
    join("node", remotePlatform, "node"),
    join("node-pty", remotePlatform, "pty.node"),
    join("glm", remotePlatform, "zcode.cjs"),
    join("tools", remotePlatform, "bfs", "bfs"),
    join("tools", remotePlatform, "ripgrep", "rg"),
    join("tools", remotePlatform, "ugrep", "ugrep"),
  ];
}

async function fetchOrNull(url: string, label: string): Promise<Response | null> {
  try {
    const response = await fetch(url);
    if (response.ok) {
      return response;
    }
    // 404 是候选路径不存在的正常分支；其它状态码说明镜像/权限问题，直接失败更可诊断。
    if (response.status === 404) {
      return null;
    }
    throw new Error(`[prepare-bundled-remote-assets] ${label} failed: HTTP ${response.status} ${url}`);
  } catch (error) {
    if (error instanceof Error && error.message.includes("[prepare-bundled-remote-assets]")) {
      throw error;
    }
    // 网络不可达/超时按候选不可用处理，让下一个候选继续尝试。
    console.warn(`[prepare-bundled-remote-assets] ${label} unreachable: ${url} (${String(error)})`);
    return null;
  }
}

async function downloadManifest(
  baseCandidates: string[],
): Promise<{ manifest: RemoteAssetManifest; manifestFileName: string; releaseBase: string }> {
  const manifestFileName = `manifest-${remotePlatform}.json`;
  // URL 拼装与运行时同源：组件版本含 `+`（如 v1.3.0+abcd）时会按路径段编码成 %2B，
  // 手写模板会在 CDN 侧 404。
  const manifestUrlCandidates = buildReleaseAssetUrlCandidates(
    buildReleaseBaseCandidates(baseCandidates, stagedVersion),
    [manifestFileName],
  );

  for (const url of manifestUrlCandidates) {
    const response = await fetchOrNull(url, "manifest request");
    if (!response) {
      continue;
    }

    const manifest = (await response.json()) as RemoteAssetManifest;
    // 只维护一个版本号：CDN 上该版本的制品必须就是当前 App 版本，否则说明版本没有对齐。
    if (manifest.appVersion !== stagedVersion || manifest.platformArch !== remotePlatform) {
      throw new Error(
        `[prepare-bundled-remote-assets] manifest identity mismatch: ` +
          `expected ${stagedVersion}/${remotePlatform}, got ${manifest.appVersion}/${manifest.platformArch} (${url})`,
      );
    }
    console.log(`[prepare-bundled-remote-assets] manifest: ${url}`);
    return { manifest, manifestFileName, releaseBase: url.slice(0, -manifestFileName.length - 1) };
  }

  throw new Error(
    `[prepare-bundled-remote-assets] 未找到 ${stagedVersion} 的 ${remotePlatform} manifest。` +
      `候选=${manifestUrlCandidates.join(", ")}。该版本的 remote 制品需要先发布到 CDN；` +
      `也可以设置 ESCODE_REMOTE_ASSET_CDN_BASE_URL 指向同一份发布的镜像。`,
  );
}

async function downloadComponentArchive(
  component: RemoteAssetManifestComponent,
  releaseBase: string,
  tempDir: string,
): Promise<string> {
  const artifactPath = normalizeRemoteAssetRelativePath(component.artifactPath, "artifactPath");
  // 组件归档上传在跨版本的 components 根目录下（不在版本目录内），
  // 版本目录候选只作为历史布局兜底；候选顺序与运行时一致。
  const urlCandidates = buildComponentArtifactUrlCandidates(
    [releaseBase],
    artifactPath,
    stagedVersion,
  );

  for (const url of urlCandidates) {
    const response = await fetchOrNull(url, `component ${component.id} request`);
    if (!response) {
      continue;
    }

    const buffer = Buffer.from(await response.arrayBuffer());
    const actualSha256 = createHash("sha256").update(buffer).digest("hex");
    if (actualSha256 !== component.sha256.toLowerCase()) {
      throw new Error(
        `[prepare-bundled-remote-assets] component ${component.id} sha256 mismatch: ` +
          `expected ${component.sha256}, got ${actualSha256} (${url})`,
      );
    }

    const archivePath = join(tempDir, `${component.id}.tar.gz`);
    await writeFile(archivePath, buffer);
    return archivePath;
  }

  throw new Error(
    `[prepare-bundled-remote-assets] 组件归档下载失败: ${component.id} (${urlCandidates.join(", ")})`,
  );
}

async function assertRequiredEntriesPresent(): Promise<void> {
  const missing: string[] = [];
  for (const relativePath of resolveRequiredEntries()) {
    try {
      await stat(join(releaseDir, relativePath));
    } catch {
      missing.push(relativePath);
    }
  }
  if (missing.length > 0) {
    throw new Error(
      `[prepare-bundled-remote-assets] staged release incomplete for ${remotePlatform}: ` +
        `missing=${missing.join(", ")} (${releaseDir})`,
    );
  }
}

async function main(): Promise<void> {
  const baseCandidates = resolveRemoteCdnReleaseBaseCandidates(process.env);
  const { manifest, manifestFileName, releaseBase } = await downloadManifest(baseCandidates);

  // 旧版本目录会随版本号累积；只保留当前版本，避免上一次构建的旧资源被一起打进安装包。
  await rm(releaseDir, { recursive: true, force: true });
  await mkdir(releaseDir, { recursive: true });

  // 运行时的本地源会读 release 目录里的 manifest（mock 分支据此做组件身份判断），
  // 少了它连接时会退回 CDN 请求。
  await writeFile(
    join(releaseDir, manifestFileName),
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );

  const tempDir = await mkdtemp(join(tmpdir(), "escode-bundled-remote-assets-"));
  try {
    for (const component of manifest.components) {
      const mount = normalizeRemoteAssetRelativePath(component.mount, `component ${component.id} mount`);
      const archivePath = await downloadComponentArchive(component, releaseBase, tempDir);
      const mountDir = join(releaseDir, ...mount.split("/"));
      await extractTarGzArchive(archivePath, mountDir);
      console.log(`[prepare-bundled-remote-assets] staged ${component.id} -> ${mount}`);
    }
  } finally {
    await rm(tempDir, { recursive: true, force: true });
  }

  await assertRequiredEntriesPresent();
  console.log(
    `[prepare-bundled-remote-assets] staged ${remotePlatform} remote release ${stagedVersion}` +
      ` -> ${releaseDir} (app ${stagedVersion})`,
  );
}

await main();
