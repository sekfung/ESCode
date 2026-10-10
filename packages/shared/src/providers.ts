import { z } from "zod";

/**
 * ESCode agent 提供方的单一真源。
 *
 * 类型 ESCodeProvider、运行时 schema escodeProviderSchema 都从这里派生,
 * 避免各处内联 z.enum([...]) 副本随新增/删除 provider 漂移。
 * 本模块只依赖 zod(叶子),可被 validation / escode-protocol 等无环引用。
 */
const ESCODE_PROVIDERS = ["glm"] as const;

export const escodeProviderSchema = z.enum(ESCODE_PROVIDERS);

export type ESCodeProvider = (typeof ESCODE_PROVIDERS)[number];
