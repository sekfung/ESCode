#!/usr/bin/env node
// 发布门禁（docs/desktop/release-artifact-hygiene.md）：release 产出的安装包必须是正式版——
// 生产后端与正式身份（文件名不带 _TEST / Preview），且应用内不带 sourcemap 文件或 sourceMappingURL 入口。
// 只读检查 electron-builder 输出目录，不修改产物；任何一项不满足即以非零退出。

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { basename, extname, join, resolve } from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
const INSTALLER_EXTENSIONS = [".exe", ".dmg", ".zip", ".AppImage", ".deb", ".rpm", ".pkg.tar.zst"];
const SCRIPT_EXTENSIONS = new Set([".js", ".cjs", ".mjs", ".css"]);
// 与 packaged-sourcemap-cleanup.mjs 同一判据：只认行首的 sourceMappingURL 注释（压缩产物可能在字符串里内嵌该文本）。
const SOURCE_MAPPING_URL_LINE_RE = /(?:^|\r?\n)[ \t]*\/\/[#@][ \t]*sourceMappingURL=/;
const SOURCE_MAPPING_URL_BLOCK_RE = /(?:^|\r?\n)[ \t]*\/\*[#@][ \t]*sourceMappingURL=/;

/** 安装包文件名：不得带测试后端后缀 `_TEST`，也不得是 Preview 身份。 */
export function findNonReleaseArtifactNames(fileNames) {
  return fileNames.filter(
    (name) =>
      INSTALLER_EXTENSIONS.some((extension) => name.endsWith(extension)) &&
      (/_TEST\b/u.test(name) || /preview/iu.test(name)),
  );
}

export function hasSourceMappingUrl(source) {
  return SOURCE_MAPPING_URL_LINE_RE.test(source) || SOURCE_MAPPING_URL_BLOCK_RE.test(source);
}

/** 文件路径清单里的 sourcemap 问题：`.map` 文件，或脚本带 sourceMappingURL 入口。 */
export function findSourcemapLeaks(entries) {
  const leaks = [];
  for (const { path, read } of entries) {
    if (extname(path) === ".map") {
      leaks.push(`${path} (sourcemap file)`);
      continue;
    }
    if (SCRIPT_EXTENSIONS.has(extname(path)) && hasSourceMappingUrl(read())) {
      leaks.push(`${path} (sourceMappingURL)`);
    }
  }
  return leaks;
}

function walk(dir, visit) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path, visit);
    else if (entry.isFile()) visit(path);
  }
}

/** electron-builder 输出里的 resources 目录（win-unpacked / linux-unpacked / mac*\/*.app/Contents/Resources）。 */
function findPackagedResourcesDirs(distDir) {
  const result = [];
  const search = (dir, depth) => {
    if (depth > 5 || !existsSync(dir)) return;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const path = join(dir, entry.name);
      if (entry.name === "resources" || entry.name === "Resources") {
        if (existsSync(join(path, "app.asar"))) {
          result.push(path);
          continue;
        }
      }
      search(path, depth + 1);
    }
  };
  search(distDir, 0);
  return result;
}

/** 归档内条目对外报告的路径。
 *  asar 的键按创建平台用分隔符（Windows 上是 `\`，listPackage 返回原生形式，statFile /
 *  extractFile 也按 path.sep 拆分），报告路径统一成 `/`，不随打包平台变化。 */
export function asarEntryPath(file) {
  return `app.asar/${file.replace(/^[\\/]+/u, "").split(/[\\/]/u).join("/")}`;
}

function collectResourcesEntries(resourcesDir) {
  const asar = require("@electron/asar");
  const entries = [];
  const appAsar = join(resourcesDir, "app.asar");
  for (const file of asar.listPackage(appAsar, { isPack: false })) {
    const relative = file.replace(/^[\\/]/u, "");
    let isFile = false;
    try {
      isFile = !asar.statFile(appAsar, relative, false).files;
    } catch {
      continue;
    }
    if (!isFile) continue;
    entries.push({
      path: asarEntryPath(relative),
      read: () => asar.extractFile(appAsar, relative).toString("utf8"),
    });
  }
  walk(resourcesDir, (path) => {
    if (path === appAsar) return;
    entries.push({ path, read: () => readFileSync(path, "utf8") });
  });
  return entries;
}

export function assertReleaseArtifacts({ distDir, env = process.env, logger = console }) {
  const problems = [];
  if (env.ESCODE_ENV?.trim().toLowerCase() !== "production") {
    problems.push(
      `ESCODE_ENV must be "production" for a release build (got "${env.ESCODE_ENV ?? ""}")`,
    );
  }
  if (env.ESCODE_PREVIEW_IDENTITY?.trim() === "1") {
    problems.push("ESCODE_PREVIEW_IDENTITY=1 produces a Preview build; unset it for a release");
  }
  const names = existsSync(distDir)
    ? readdirSync(distDir).filter((name) => statSync(join(distDir, name)).isFile())
    : [];
  const installers = names.filter((name) => INSTALLER_EXTENSIONS.some((ext) => name.endsWith(ext)));
  if (installers.length === 0) problems.push(`no installers found in ${distDir}`);
  for (const name of findNonReleaseArtifactNames(names)) {
    problems.push(`non-release artifact name: ${name}`);
  }
  const resourcesDirs = findPackagedResourcesDirs(distDir);
  if (resourcesDirs.length === 0)
    problems.push(`no packaged app (resources/app.asar) found under ${distDir}`);
  for (const resourcesDir of resourcesDirs) {
    for (const leak of findSourcemapLeaks(collectResourcesEntries(resourcesDir))) {
      problems.push(`sourcemap leak: ${leak}`);
    }
  }
  if (problems.length > 0) {
    throw new Error(
      `[release-artifacts] release hygiene check failed:\n- ${problems.join("\n- ")}`,
    );
  }
  logger.log(
    `[release-artifacts] ok: ${installers.map((name) => basename(name)).join(", ")}; scanned ${resourcesDirs.length} packaged app(s)`,
  );
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const distDir = resolve(process.argv[2] ?? resolve(import.meta.dirname, "..", "dist"));
  try {
    assertReleaseArtifacts({ distDir });
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
