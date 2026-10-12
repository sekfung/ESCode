// ESCode LLM Wiki —— Desktop 知识图谱协议（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11）。
//
// 只有 Rust runtime 实现这些方法；TS runtime 不响应（Host 侧按「不支持」呈现）。
// 作用域固定为 global + 当前 workspace 的项目：参数里没有项目字段，跨项目查询不可表达。
// 条目详情 `entry` 内的字段沿用知识条目 frontmatter 的 snake_case 命名（与 WikiRead 工具结果同源）。
import { z } from "zod";

export const V4_WIKI_METHODS = {
  status: "v4/wiki/status",
  graph: "v4/wiki/graph",
  entry: "v4/wiki/entry",
  manage: "v4/wiki/manage",
} as const;

export const WIKI_SCOPES = ["global", "project"] as const;
export const WIKI_KINDS = [
  "standard",
  "architecture",
  "adr",
  "contract",
  "business-rule",
  "invariant",
  "guide",
  "reference",
  "pattern",
  "anti-pattern",
  "proposal",
] as const;
export const WIKI_STATUSES = [
  "proposal",
  "approved",
  "needs-review",
  "deprecated",
  "superseded",
  "rejected",
] as const;
export const WIKI_AUTHORITIES = [
  "approved-global-standard",
  "approved-project-standard",
  "approved-adr-contract",
  "verified-reference",
  "observed-evidence",
  "historical-review",
  "proposal",
] as const;
export const WIKI_VALIDITIES = ["valid", "stale", "broken-source", "unverified"] as const;
export const WIKI_EDGE_TYPES = ["related", "supersedes", "source", "conflict"] as const;
export const WIKI_MANAGE_ACTIONS = [
  "status",
  "lint",
  "validate",
  "conflicts",
  "ingest",
  "sync",
  "approve",
  "reject",
  "deprecate",
  "promote",
  "rebuild_index",
] as const;

export const wikiScopeSchema = z.enum(WIKI_SCOPES);
export const wikiKindSchema = z.enum(WIKI_KINDS);
export const wikiStatusSchema = z.enum(WIKI_STATUSES);
export const wikiAuthoritySchema = z.enum(WIKI_AUTHORITIES);
export const wikiValiditySchema = z.enum(WIKI_VALIDITIES);
export type WikiScope = z.infer<typeof wikiScopeSchema>;
export type WikiKind = z.infer<typeof wikiKindSchema>;
export type WikiStatus = z.infer<typeof wikiStatusSchema>;

export const v4WikiProjectSchema = z.object({
  id: z.string(),
  name: z.string(),
  identityKind: z.enum(["git-remote", "git-local", "workspace-identity", "path"]).optional(),
  root: z.string().optional(),
});
export type V4WikiProject = z.infer<typeof v4WikiProjectSchema>;

const countSchema = z.number().int().nonnegative();

export const v4WikiStatusParamsSchema = z.strictObject({});
export const v4WikiStatusResultSchema = z.object({
  /** false：runtime 未组合 Wiki（或不是 Rust runtime 的兼容回答）。 */
  supported: z.boolean(),
  enabled: z.boolean(),
  autoInject: z.boolean().optional(),
  project: v4WikiProjectSchema.optional(),
  scopes: z
    .array(
      z.object({
        scope: wikiScopeSchema,
        entries: countSchema,
        byStatus: z.record(z.string(), countSchema),
        sources: z.record(z.string(), countSchema),
        indexVersion: countSchema,
        invalidFiles: countSchema,
        root: z.string(),
      }),
    )
    .optional(),
  conflicts: countSchema.optional(),
  configIssues: z.array(z.string()).optional(),
  ignoredProjectKeys: z.array(z.string()).optional(),
});
export type V4WikiStatusResult = z.infer<typeof v4WikiStatusResultSchema>;

export const V4_WIKI_GRAPH_MAX_NODES = 2000;
export const v4WikiGraphParamsSchema = z.strictObject({
  scopes: z.array(wikiScopeSchema).optional(),
  statuses: z.array(wikiStatusSchema).optional(),
  kinds: z.array(wikiKindSchema).optional(),
  includeSources: z.boolean().optional(),
  limit: z.number().int().min(1).max(V4_WIKI_GRAPH_MAX_NODES).optional(),
});
export type V4WikiGraphParams = z.infer<typeof v4WikiGraphParamsSchema>;

export const v4WikiGraphNodeSchema = z.object({
  id: z.string(),
  type: z.enum(["entry", "source"]),
  label: z.string(),
  scope: wikiScopeSchema,
  entryId: z.string().optional(),
  kind: wikiKindSchema.optional(),
  status: wikiStatusSchema.optional(),
  authority: wikiAuthoritySchema.optional(),
  validity: wikiValiditySchema.optional(),
  format: z.string().optional(),
  sourceStatus: z.string().optional(),
});
export type V4WikiGraphNode = z.infer<typeof v4WikiGraphNodeSchema>;

export const v4WikiGraphEdgeSchema = z.object({
  id: z.string(),
  from: z.string(),
  to: z.string(),
  type: z.enum(WIKI_EDGE_TYPES),
});
export type V4WikiGraphEdge = z.infer<typeof v4WikiGraphEdgeSchema>;

export const v4WikiGraphResultSchema = z.object({
  enabled: z.boolean(),
  project: v4WikiProjectSchema,
  nodes: z.array(v4WikiGraphNodeSchema),
  edges: z.array(v4WikiGraphEdgeSchema),
  truncated: z.boolean(),
  counts: z.record(z.string(), countSchema),
});
export type V4WikiGraphResult = z.infer<typeof v4WikiGraphResultSchema>;

export const v4WikiEntryParamsSchema = z.strictObject({ id: z.string().min(1).max(160) });

const wikiConflictSchema = z.object({
  kind: z.string(),
  parties: z.array(
    z.object({
      id: z.string(),
      scope: wikiScopeSchema,
      status: z.string(),
      authority: z.string(),
      updated_at: z.string(),
      sources: z.array(z.string()),
    }),
  ),
  evidence: z.string(),
  suggestion: z.string(),
});
export type V4WikiConflict = z.infer<typeof wikiConflictSchema>;

export const v4WikiEntrySchema = z.object({
  id: z.string(),
  title: z.string(),
  scope: wikiScopeSchema,
  kind: wikiKindSchema,
  status: wikiStatusSchema,
  authority: wikiAuthoritySchema,
  validity: wikiValiditySchema,
  validity_reason: z.string().nullable().optional(),
  origin: z.string(),
  tags: z.array(z.string()),
  related: z.array(z.string()),
  supersedes: z.string().nullable().optional(),
  superseded_by: z.string().nullable().optional(),
  applies_to: z.array(z.string()).optional(),
  updated_at: z.string(),
  created_at: z.string().optional(),
  body: z.string(),
  sources: z.array(
    z.object({
      document_id: z.string(),
      source_path: z.string(),
      format: z.string(),
      locator: z.string(),
      excerpt: z.string().nullable().optional(),
      content_hash: z.string(),
      source_status: z.string(),
    }),
  ),
});
export type V4WikiEntry = z.infer<typeof v4WikiEntrySchema>;

export const v4WikiEntryResultSchema = z.object({
  found: z.boolean(),
  entry: v4WikiEntrySchema.optional(),
  conflicts: z.array(wikiConflictSchema).optional(),
  review: z
    .array(
      z.object({ action: z.string(), at: z.string(), by: z.string(), note: z.string().optional() }),
    )
    .optional(),
});
export type V4WikiEntryResult = z.infer<typeof v4WikiEntryResultSchema>;

export const v4WikiManageParamsSchema = z.strictObject({
  action: z.enum(WIKI_MANAGE_ACTIONS),
  ids: z.array(z.string().min(1).max(160)).max(200).optional(),
  paths: z.array(z.string().min(1).max(4096)).max(200).optional(),
  scope: wikiScopeSchema.optional(),
  reason: z.string().max(2000).optional(),
  supersededBy: z.string().max(160).optional(),
  approveImported: z.boolean().optional(),
});
export type V4WikiManageParams = z.infer<typeof v4WikiManageParamsSchema>;

export const v4WikiManageResultSchema = z.object({
  action: z.enum(WIKI_MANAGE_ACTIONS),
  ok: z.boolean(),
  summary: z.string(),
  items: z.array(z.unknown()),
});
export type V4WikiManageResult = z.infer<typeof v4WikiManageResultSchema>;
