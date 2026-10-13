import { randomUUID } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

function normalizePath(file) {
  return file.replaceAll("\\", "/");
}

function shellQuote(value) {
  if (process.platform === "win32") {
    // Bugfix: Windows 的 pre-push 路径会在 lint-staged 临时 GIT_INDEX_FILE 下 spawn
    // 返回的命令字符串；POSIX 单引号在这里可能被当成可执行参数的一部分，导致任务无法启动。
    return `"${value.replaceAll('"', '\\"')}"`;
  }

  return `'${value.replaceAll("'", "'\\''")}'`;
}

function quoteFiles(files) {
  return files.map(shellQuote).join(" ");
}

function writeFilesManifest(mode, files) {
  const manifestPath = `.lint-staged-vitest-${process.pid}-${randomUUID()}.json`;
  writeFileSync(
    manifestPath,
    JSON.stringify({
      mode,
      files,
    }),
  );
  return manifestPath;
}

function vitestCommand(mode, files) {
  // 不按文件拆批：重复启动 Vitest 会增加开销，related 拆批还会重复执行共同依赖的测试。
  // Bugfix: Windows 下 lint-staged 返回的是 shell command string，文件名里的 %PATH%
  // 会先被 cmd.exe 展开。文件列表写入 manifest，只把安全生成的 manifest 路径传给 Node。
  const manifestPath = writeFilesManifest(mode, files);
  return `node scripts/run-lint-staged-vitest.mjs ${mode} --files-manifest ${quoteFiles([
    manifestPath,
  ])}`;
}

function isRootUnitTestFile(file) {
  return (
    /^packages\/[^/]+\/test\/.*\.test\.ts$/u.test(file) &&
    !file.startsWith("packages/desktop/test/e2e/")
  );
}

function isRootVitestSourceFile(file) {
  return (
    file.startsWith("packages/") &&
    !file.startsWith("packages/desktop/test/e2e/") &&
    !isRootUnitTestFile(file)
  );
}

/** 纯文案目录。 */
const I18N_LOCALE_FILE = /^packages\/[^/]+\/src\/i18n\/locales\//u;
/** 直接 import locale 模块的用例，逐条断言词条内容。 */
const I18N_LOCALE_IMPORT = /from\s+["'][^"']*i18n\/locales\//u;

/**
 * 找直接 import locale 模块的用例。
 *
 * 文案目录不能直接喂 `vitest related`：`packages/ui/src/i18n/IntlProvider` 被三百多个 UI 测试
 * 引入，反向依赖闭包接近整个 UI 套件，单次 pre-push 要多跑几百个文件；但也不能干脆不跑——
 * 这些直接 import locale 的用例逐条断言词条（如 remoteSyncLocale），正是改文案时的哨兵，
 * 数量只有几十个。按文件内容现算，避免写死路径之后随重构失效。
 */
function findI18nLocaleGuards() {
  const guards = [];

  const visit = (dir) => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const filePath = join(dir, entry.name);
      if (entry.isDirectory()) {
        visit(filePath);
      } else if (
        entry.name.endsWith(".test.ts") &&
        I18N_LOCALE_IMPORT.test(readFileSync(filePath, "utf8"))
      ) {
        guards.push(normalizePath(filePath));
      }
    }
  };

  for (const entry of readdirSync("packages", { withFileTypes: true })) {
    if (entry.isDirectory()) {
      visit(join("packages", entry.name, "test"));
    }
  }

  return guards.sort();
}

export default {
  "packages/*/test/**/*.test.ts": (files) => {
    const testFiles = files.map(normalizePath).filter(isRootUnitTestFile);

    if (testFiles.length === 0) {
      return [];
    }

    return vitestCommand("run", testFiles);
  },
  "packages/**/*.{js,jsx,ts,tsx,json}": (files) => {
    const sourceFiles = files.map(normalizePath).filter(isRootVitestSourceFile);

    if (sourceFiles.length === 0) {
      return [];
    }

    if (!sourceFiles.some((file) => I18N_LOCALE_FILE.test(file))) {
      return vitestCommand("related", sourceFiles);
    }

    // 文案文件换成它们的直接导入者，仍然只发一条命令（只启动一次 Vitest）。
    const seeds = [
      ...sourceFiles.filter((file) => !I18N_LOCALE_FILE.test(file)),
      ...findI18nLocaleGuards(),
    ];

    return seeds.length === 0 ? [] : vitestCommand("related", seeds);
  },
};
