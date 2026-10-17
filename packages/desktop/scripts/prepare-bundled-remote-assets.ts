#!/usr/bin/env node
/**
 * 构建期把「构建目标架构对应的 Linux 远程 release」组装到 bundled-remote-assets，
 * 供 electron-builder 复制进 resources/remote-assets。
 *
 * 来源是本仓库源码构建的 mock-cdn release（prepare:remote-assets → scripts/prepare-prebuilds.mjs），
 * 不再从 CDN 下载上游 ZCode 制品（docs/specs/offline-remote-assets.md §7「制品来源」）：
 * 更名后本地侧契约（server bundle 能力标记、官方插件 .escode-plugin 目录与包名）与上游制品不一致，
 * 内置上游制品会让每次远程连接都重传 server，并让 glm 被判不完整而回退 CDN。
 * 产物只存在于构建工作区（mock-cdn、bundled-remote-assets 已 gitignore），不需要入库。
 *
 * 平台映射（见 docs/specs/offline-remote-assets.md）：远程后端只支持 POSIX，且 Windows 桌面的
 * 远程目标（WSL / SSH / Docker）都是 Linux，所以按构建目标的架构取同架构的 linux release：
 * x64 -> linux-x64，arm64 -> linux-arm64。
 */
import { createHash } from "node:crypto";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { REMOTE_AGENT_OFFICIAL_PLUGIN_REQUIRED_RELATIVE_PATHS } from "@escode/server/remote/escodeAgentOfficialPluginAssets.js";
import { extractTarGzArchive } from "@escode/server/remote/localTarGz.js";
import { normalizeRemoteAssetRelativePath } from "@escode/server/remote/remoteAssetCdn.js";
import { REQUIRED_SERVER_BUNDLE_MARKERS } from "@escode/server/remote/serverBundleDeployCheck.js";
import { REMOTE_AGENT_RUST_BINARY_NAME } from "@escode/server/remote/escodeAgentBundleWrapper.js";
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
// 测试用夹具目录覆盖；缺省即 prepare:remote-assets 的输出目录。
const sourceMockCdnDir = resolve(
  process.env["ESCODE_BUNDLED_REMOTE_ASSETS_SOURCE_DIR"]?.trim() || join(desktopDir, "mock-cdn"),
);

// 目录名必须与运行时 ESCODE_VERSION 逐字一致：主进程按
// <resources>/remote-assets/releases/<ESCODE_VERSION> 判断内置资源是否可用，名字对不上会静默退回 CDN。
const stagedVersion = getBuildMetadata().appVersion;
const remotePlatform = `linux-${getTargetPlatform().arch}`;
const releaseDir = join(bundledDir, "releases", stagedVersion);
const manifestFileName = `manifest-${remotePlatform}.json`;

/**
 * 必须存在的条目：与运行时 findMissingMockReleasePaths 的组件必需路径保持一致（含 glm 官方插件资产）。
 * Bugfix：此前漏了官方插件路径，上游 .zcode-plugin 制品因此通过了构建期校验，安装后才在连接时被判不完整。
 */
function resolveRequiredEntries(): string[] {
  return [
    join("server", "zcode-server.cjs"),
    join("node", remotePlatform, "node"),
    join("node-pty", remotePlatform, "pty.node"),
    join("glm", remotePlatform, "zcode.cjs"),
    join("glm", remotePlatform, REMOTE_AGENT_RUST_BINARY_NAME),
    ...REMOTE_AGENT_OFFICIAL_PLUGIN_REQUIRED_RELATIVE_PATHS.map((relativePath) =>
      join("glm", remotePlatform, "packages", ...relativePath.split("/")),
    ),
    join("tools", remotePlatform, "bfs", "bfs"),
    join("tools", remotePlatform, "ripgrep", "rg"),
    join("tools", remotePlatform, "ugrep", "ugrep"),
  ];
}

async function readSourceManifest(): Promise<RemoteAssetManifest> {
  const manifestPath = join(sourceMockCdnDir, "releases", stagedVersion, manifestFileName);
  let raw: string;
  try {
    raw = await readFile(manifestPath, "utf8");
  } catch {
    throw new Error(
      `[prepare-bundled-remote-assets] 未找到本地 ${stagedVersion} 的 ${remotePlatform} manifest: ${manifestPath}。` +
        "请先运行 prepare:remote-assets（不要设置 ESCODE_SKIP_REMOTE_ASSETS=1），" +
        `且 ESCODE_REMOTE_ASSET_PLATFORMS 需包含 ${remotePlatform}。`,
    );
  }
  const manifest = JSON.parse(raw) as RemoteAssetManifest;
  // 本地源由同一仓库版本构建；身份不符说明 mock-cdn 残留了其他版本或平台的产物。
  if (manifest.appVersion !== stagedVersion || manifest.platformArch !== remotePlatform) {
    throw new Error(
      `[prepare-bundled-remote-assets] manifest identity mismatch: ` +
        `expected ${stagedVersion}/${remotePlatform}, got ${manifest.appVersion}/${manifest.platformArch} (${manifestPath})`,
    );
  }
  console.log(`[prepare-bundled-remote-assets] manifest: ${manifestPath}`);
  return manifest;
}

async function stageComponent(component: RemoteAssetManifestComponent): Promise<void> {
  const artifactPath = normalizeRemoteAssetRelativePath(component.artifactPath, "artifactPath");
  const mount = normalizeRemoteAssetRelativePath(
    component.mount,
    `component ${component.id} mount`,
  );
  // artifactPath 相对 mock-cdn 根目录（与 CDN 上跨版本的 components 根布局一致）。
  const archivePath = join(sourceMockCdnDir, ...artifactPath.split("/"));
  let buffer: Buffer;
  try {
    buffer = await readFile(archivePath);
  } catch {
    throw new Error(
      `[prepare-bundled-remote-assets] 组件归档缺失: ${component.id} (${archivePath})`,
    );
  }
  const actualSha256 = createHash("sha256").update(buffer).digest("hex");
  if (actualSha256 !== component.sha256.toLowerCase()) {
    throw new Error(
      `[prepare-bundled-remote-assets] component ${component.id} sha256 mismatch: ` +
        `expected ${component.sha256}, got ${actualSha256} (${archivePath})`,
    );
  }
  await extractTarGzArchive(archivePath, join(releaseDir, ...mount.split("/")));
  console.log(`[prepare-bundled-remote-assets] staged ${component.id} -> ${mount}`);
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

/** 与远端 checkServerBundleRequiredMarkers 同口径：缺标记的 server 每次连接都会被判定需要重传。 */
async function assertServerBundleMarkersPresent(): Promise<void> {
  const serverBundlePath = join(releaseDir, "server", "zcode-server.cjs");
  const content = await readFile(serverBundlePath, "utf8");
  const missing = REQUIRED_SERVER_BUNDLE_MARKERS.filter((marker) => !content.includes(marker));
  if (missing.length > 0) {
    throw new Error(
      `[prepare-bundled-remote-assets] server bundle missing required markers: ${missing.join(",")} (${serverBundlePath})`,
    );
  }
}

async function main(): Promise<void> {
  const manifest = await readSourceManifest();

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

  for (const component of manifest.components) {
    await stageComponent(component);
  }

  await assertRequiredEntriesPresent();
  await assertServerBundleMarkersPresent();
  console.log(
    `[prepare-bundled-remote-assets] staged ${remotePlatform} remote release ${stagedVersion}` +
      ` -> ${releaseDir} (source ${sourceMockCdnDir})`,
  );
}

await main();
