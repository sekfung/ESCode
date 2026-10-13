import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { isGatedByPrefix } from "../../../../../scripts/generate-rea-auto-approved-tools.mjs";
import { resolveBuiltInReaMcpServers } from "../src/app/built-in-rea.js";
import {
  REA_AUTO_APPROVED_TOOLS,
  REA_AUTO_APPROVED_TOOLS_VERSION,
} from "../src/app/rea-auto-approved-tools.js";
import {
  OFFICIAL_PLUGIN_DEFINITIONS,
  OFFICIAL_REA_PLUGIN_ID,
} from "../src/app/official-plugin-definitions.js";

const packagesRoot = fileURLToPath(new URL("../../", import.meta.url));
const pluginRoot = join(packagesRoot, "rea-plugin");

function readJson(relativePath: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(pluginRoot, relativePath), "utf8")) as Record<string, unknown>;
}

/**
 * REA 是仓库里第一个把约 100 MB 第三方运行时随包分发的内置插件。它的几个约束无法靠类型表达，
 * 一旦被改坏会静默失效（运行时进不了缓存、或表与版本漂移），所以在这里逐条钉住。
 */
describe("rea official plugin", () => {
  const definition = OFFICIAL_PLUGIN_DEFINITIONS.find((entry) => entry.name === "rea");

  it("is registered as an opt-in plugin whose MCP server is host-injected", () => {
    expect(definition?.name).toBe("rea");
    expect(OFFICIAL_REA_PLUGIN_ID).toBe("rea@zcode-plugins-official");
    // 170 个工具的紧凑输入 schema 约 253 KB，交给用户按需开启。
    expect(definition?.defaultEnabled).toBeUndefined();
    // 设置页靠它把宿主注入的 server 归属到本插件。
    expect(definition?.hostMcpServerNames).toEqual(["rea"]);
  });

  it("keeps the ~100 MB runtime out of the plugin seed", () => {
    // 把 node_modules 列进 runtimeTopLevelPaths 会让 seed 每次启动 walk + sha256 全部插件文件
    // （实测这棵树 915 ms），并在首次填缓存时整棵复制 —— 实测 bootstrap 的 plugins 测试会从
    // ~3s 变成 5s 超时。运行时因此由打包链路单独 stage，宿主把路径注入给 MCP server。
    expect(definition?.runtimeTopLevelPaths).toBeUndefined();
    expect(definition?.requiredSeedPaths).toEqual([
      "dist/mcp/server.js",
      "skills/reverse-engineer-anything/SKILL.md",
    ]);
    for (const required of definition?.requiredSeedPaths ?? []) {
      expect(existsSync(join(pluginRoot, ...required.split("/"))), required).toBe(true);
    }
  });

  it("never declares mcpServers in the manifest (seed would rewrite our entry)", () => {
    const manifest = readJson(join(".escode-plugin", "plugin.json"));
    // seed 会把内置插件 manifest 的 mcpServers 无条件改写成 <root>/dist/mcp/server.js，
    // 那样就丢掉了宿主注入的 env（compact profile 等）。server 必须保持宿主注入。
    expect(manifest.mcpServers).toBeUndefined();
    expect(manifest.skills).toBe("skills");
  });

  it("keeps the auto-approved allowlist aligned with the pinned REA version and the rule", () => {
    const packageJson = readJson("package.json") as { escodePlugin?: { reaAgentsVersion?: string } };
    // 清单是生成物（scripts/generate-rea-auto-approved-tools.mjs）：版本必须与 pin 一致，
    // 否则 REA 升级后清单过期会被这里拦下。
    expect(REA_AUTO_APPROVED_TOOLS_VERSION).toBe(packageJson.escodePlugin?.reaAgentsVersion);
    expect(definition?.defaultAllowedTools).toEqual(REA_AUTO_APPROVED_TOOLS);
    expect(REA_AUTO_APPROVED_TOOLS.length).toBeGreaterThan(0);
    // 反向断言：任何命中"必须审批"前缀的名字都不该出现在清单里（手改放宽会在这里失败）。
    for (const name of REA_AUTO_APPROVED_TOOLS) {
      expect(isGatedByPrefix(name), name).toBe(false);
    }
    // 破坏性与写/会话类必须逐次审批。
    for (const gated of [
      "close_binary",
      "export_evidence_bundle",
      "unset_bookmark",
      "observe_web_execution",
      "open_binary",
      "set_comment",
      "install_adb_package",
      "capture_web_screenshot",
    ]) {
      expect(REA_AUTO_APPROVED_TOOLS, gated).not.toContain(gated);
    }
    // REA 自标只读的 5 个必须在清单里（它们不靠前缀规则命中）。
    for (const annotated of [
      "binary_session",
      "evaluate_reconstruction_coverage",
      "get_evidence_bundle",
      "list_unknowns",
      "verify_unknown_resolution",
    ]) {
      expect(REA_AUTO_APPROVED_TOOLS, annotated).toContain(annotated);
    }
  });

  it("maps userConfig engine paths to REA env vars", () => {
    // 可选增强（Ghidra/Hopper/IDA/JDK/WABT/JEB/ILSpy…）不随包：用户在插件设置里填路径，
    // 宿主把它映射成 REA 的 env。映射规则是选项键 → SCREAMING_SNAKE，不许改成手写对照表。
    const servers = resolveBuiltInReaMcpServers({
      pluginOutcome: {
        plugins: [
          {
            configuredOptions: {
              extraEnvJson: '{"REA_GHIDRA_LANGUAGE_ID":"x86:LE:64:default","IGNORED":"  "}',
              ghidraInstallDir: " /opt/ghidra ",
              javaHome: "/usr/lib/jvm/temurin-21",
              reaAnalysisProvider: "ghidra",
            },
            enabled: true,
            id: OFFICIAL_REA_PLUGIN_ID,
            rootPath: pluginRoot,
            userConfig: {
              hopperLauncherPath: { default: "/Applications/Hopper Disassembler.app/Contents/MacOS/hopper" },
              reaWabtBinDirectory: {},
            },
          },
        ],
      } as never,
      workingDirectory: pluginRoot,
    });

    const env = servers.rea?.env ?? {};
    expect(env.GHIDRA_INSTALL_DIR).toBe("/opt/ghidra");
    expect(env.JAVA_HOME).toBe("/usr/lib/jvm/temurin-21");
    expect(env.REA_ANALYSIS_PROVIDER).toBe("ghidra");
    // 未填写（配置项存在但既无值也无默认）的项不注入，交给 REA 自己探测与报错。
    expect(env.REA_WABT_BIN_DIRECTORY).toBeUndefined();
    // 默认值补位：用户没填但 manifest 有默认时按默认注入。
    expect(env.HOPPER_LAUNCHER_PATH).toContain("Hopper");
    // 逃生口：JSON 对象里的键值直接并入 env，空白值不注入。
    expect(env.REA_GHIDRA_LANGUAGE_ID).toBe("x86:LE:64:default");
    expect(env.IGNORED).toBeUndefined();
    expect(env.EXTRA_ENV_JSON).toBeUndefined();
    // 固定项不被覆盖。
    expect(env.REA_MCP_INPUT_SCHEMA_PROFILE).toBe("compact");
  });

  it("ships an ES module entry and an auto-loadable skill", () => {
    const packageJson = readJson("package.json") as { type?: string; license?: string };
    // 入口是 ESM；缺 type: module 时 Node 会重新解析并打 MODULE_TYPELESS_PACKAGE_JSON 警告。
    expect(packageJson.type).toBe("module");
    expect(packageJson.license).toBe("MIT");
    const skill = readFileSync(
      join(pluginRoot, "skills", "reverse-engineer-anything", "SKILL.md"),
      "utf8",
    ).replaceAll("\r\n", "\n");
    expect(skill.startsWith("---\n")).toBe(true);
    expect(skill).toContain("name: reverse-engineer-anything");
    for (const reference of [
      "android-applications.md",
      "connection-and-recovery.md",
      "evidence-workflows.md",
      "javascript-applications.md",
      "native-and-artifacts.md",
      "runtime-observation.md",
    ]) {
      expect(
        existsSync(join(pluginRoot, "skills", "reverse-engineer-anything", "references", reference)),
        reference,
      ).toBe(true);
    }
  });
});
