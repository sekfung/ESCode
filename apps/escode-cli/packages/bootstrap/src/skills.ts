import { resolve } from "node:path";
<<<<<<< HEAD:apps/escode-cli/packages/bootstrap/src/skills.ts
import { createConfig, resolvePath } from "@escode/adapters/config";
import { createNodeSkillAdapter } from "@escode/adapters/skills";
import type { Logger, SkillContent, SkillDiagnostic, SkillLoadOutcome } from "@escode/contracts";
import { resolveBundledSkillRoots } from "./app/bundled-skills.js";
import { getCliStorageRoot } from "./app/paths.js";
import { resolveESCodePlugins } from "./plugins.js";
=======
import { createConfig, resolvePath } from "@zcode/adapters/config";
import { createNodeSkillAdapter } from "@zcode/adapters/skills";
import type { Logger, SkillContent, SkillDiagnostic, SkillLoadOutcome } from "@zcode/contracts";
import { resolveBundledSkillRoots } from "./app/bundled-skills.js";
import { getCliStorageRoot } from "./app/paths.js";
import { filterVisualizeSkillRoots } from "./app/visualize-skill-gate.js";
import { resolveZCodePlugins } from "./plugins.js";
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/bootstrap/src/skills.ts
import { collectDisabledPaths } from "./skill-command-overrides.js";

export interface ListESCodeSkillsOptions {
  env?: NodeJS.ProcessEnv;
  /** 协议宿主草稿目录使用；CLI 命令缺省关闭。 */
  includeVisualize?: boolean;
  logger?: Logger;
  projectConfigPath?: string;
  skipUserConfig?: boolean;
  userConfigPath?: string;
  workingDirectory?: string;
}

export interface InspectESCodeSkillOptions extends ListESCodeSkillsOptions {
  name: string;
}

export interface ESCodeSkillInspection {
  diagnostics: SkillDiagnostic[];
  skill: SkillContent;
}

export async function listESCodeSkills(
  options: ListESCodeSkillsOptions = {},
): Promise<SkillLoadOutcome> {
  const discovery = await createSkillDiscovery(options);
  if (!discovery.enabled) {
    return {
      diagnostics: [],
      skills: [],
      totalDiscovered: 0,
    };
  }

  return await discovery.skillPort.discoverSkills({
    workingDirectory: discovery.workingDirectory,
  });
}

export async function inspectESCodeSkill(
  options: InspectESCodeSkillOptions,
): Promise<ESCodeSkillInspection> {
  const discovery = await createSkillDiscovery(options);
  if (!discovery.enabled) {
    throw new Error("Skills are disabled.");
  }

  const outcome = await discovery.skillPort.discoverSkills({
    workingDirectory: discovery.workingDirectory,
  });
  if (
    !outcome.skills.some(
      (skill) => skill.name === options.name || skill.qualifiedName === options.name,
    )
  ) {
    throw new Error(`Skill not found: ${options.name}`);
  }

  const skill = await discovery.skillPort.loadSkill({
    name: options.name,
    workingDirectory: discovery.workingDirectory,
  });

  return {
    diagnostics: outcome.diagnostics,
    skill,
  };
}

async function createSkillDiscovery(options: ListESCodeSkillsOptions): Promise<
  | {
      enabled: false;
      workingDirectory: string;
    }
  | {
      enabled: true;
      skillPort: ReturnType<typeof createNodeSkillAdapter>;
      workingDirectory: string;
    }
> {
  const workingDirectory = resolve(options.workingDirectory ?? process.cwd());
  const configResult = createConfig({
    env: options.env,
    projectConfigPath: options.projectConfigPath,
    workingDirectory,
    skipUserConfig: options.skipUserConfig,
    userConfigPath: options.userConfigPath,
  });

  if (!configResult.config.features.skill || !configResult.config.skills.enabled) {
    return {
      enabled: false,
      workingDirectory,
    };
  }
  const pluginOutcome = resolveESCodePlugins({
    configResult,
    env: options.env,
    logger: options.logger,
    projectConfigPath: options.projectConfigPath,
    skipUserConfig: options.skipUserConfig,
    userConfigPath: options.userConfigPath,
    workingDirectory,
  });

<<<<<<< HEAD:apps/escode-cli/packages/bootstrap/src/skills.ts
  // 内置技能包与插件技能根并列注入：`escode skills list`、引用目录与 runtime 看到同一份发现结果。
  const bundledSkillRoots = await resolveBundledSkillRoots({
=======
  // 内置技能包与插件技能根并列注入：`zcode skills list`、引用目录与 runtime 看到同一份发现结果。
  const bundledSkillRoots = resolveBundledSkillRoots({
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/bootstrap/src/skills.ts
    cliStorageRoot: getCliStorageRoot(resolvePath(configResult.config.storage.dir)),
    logger: options.logger,
  });

  return {
    enabled: true,
    skillPort: createNodeSkillAdapter({
      extraRoots: configResult.config.skills.roots,
<<<<<<< HEAD:apps/escode-cli/packages/bootstrap/src/skills.ts
      extraResolvedRoots: [...pluginOutcome.skillRoots, ...bundledSkillRoots],
=======
      extraResolvedRoots: [
        ...filterVisualizeSkillRoots(pluginOutcome.skillRoots, options.includeVisualize),
        ...bundledSkillRoots,
      ],
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/bootstrap/src/skills.ts
      disabledPaths: collectDisabledPaths(configResult.config.skillOverrides),
    }),
    workingDirectory,
  };
}
