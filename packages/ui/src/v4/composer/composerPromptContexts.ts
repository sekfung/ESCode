import {
  buildPromptWithCodeComments,
  parsePromptCodeComments,
  type CodeCommentComposerAttachment,
} from "@/lib/codeCommentContext.js";
import { stripGenUiModelContext } from "@escode/shared/gen-ui";
import {
  buildPromptWithConversationSelections,
  parsePromptConversationSelections,
  type ConversationSelectionDisplayReference,
} from "@/lib/conversationSelectionReference.js";
import {
  buildPromptWithWebElementContexts,
  parsePromptWebElementContexts,
  type WebElementContextComposerAttachment,
} from "@/lib/webElementContext.js";
import {
  buildPromptWithPptxElementReferences,
  parsePromptPptxElementReferences,
  type PptxElementReference,
} from "@/lib/pptxElementReference.js";
import {
  buildPromptWithPluginUiContexts,
  parsePromptPluginUiContexts,
  type PluginUiModelContext,
} from "@/plugin-ui/index.js";
import {
  buildPromptWithCircuitReferences,
  parsePromptCircuitReferences,
} from "@/lib/circuit/circuitReferenceDrafts.js";
import type { CircuitDraftReference } from "@/circuit/domain/circuitReferences.js";

export interface ComposerPromptContexts {
  codeComments: readonly CodeCommentComposerAttachment[];
  conversationSelections: readonly ConversationSelectionDisplayReference[];
  webElements: readonly WebElementContextComposerAttachment[];
  pptxElements: readonly PptxElementReference[];
  /** 面板「插入到输入框」的电路引用 chip（docs/circuit/README.md §8）；缺省视为空。 */
  circuitReferences?: readonly CircuitDraftReference[];
  /** 插件 UI 经 ui/update-model-context 附加；缺省视为空。 */
  pluginUiContexts?: readonly PluginUiModelContext[];
}

export function countComposerPromptContexts(contexts: {
  codeComments: readonly unknown[];
  conversationSelections: readonly unknown[];
  webElements: readonly unknown[];
  pptxElements: readonly unknown[];
  circuitReferences?: readonly unknown[];
  pluginUiContexts?: readonly unknown[];
}) {
  return (
    contexts.codeComments.length +
    contexts.conversationSelections.length +
    contexts.webElements.length +
    contexts.pptxElements.length +
    (contexts.circuitReferences?.length ?? 0) +
    (contexts.pluginUiContexts?.length ?? 0)
  );
}

/**
 * 六类 context parser 都只识别 prompt 尾块，因此序列化顺序和解析顺序必须严格相反。
 * 电路引用排在 PPTX 之后，插件上下文最后序列化（最外层），解析时按相反顺序剥离。
 */
export function serializeComposerPromptContexts(
  text: string,
  contexts: ComposerPromptContexts,
): string {
  const withSelections = buildPromptWithConversationSelections(
    text,
    contexts.conversationSelections,
  );
  const withCodeComments = buildPromptWithCodeComments(
    withSelections,
    contexts.codeComments,
  );
  const withWebElements = buildPromptWithWebElementContexts(
    withCodeComments,
    contexts.webElements,
  );
  const withPptx = buildPromptWithPptxElementReferences(
    withWebElements,
    contexts.pptxElements,
  );
  const withCircuit = buildPromptWithCircuitReferences(
    withPptx,
    contexts.circuitReferences ?? [],
  );
  return buildPromptWithPluginUiContexts(withCircuit, contexts.pluginUiContexts ?? []);
}

export function parseComposerPromptContexts(
  content: string,
  workspace: { workspacePath: string; workspaceIdentity?: string },
): {
  visibleContent: string;
  codeComments: CodeCommentComposerAttachment[];
  conversationSelections: readonly ConversationSelectionDisplayReference[];
  webElements: WebElementContextComposerAttachment[];
  pptxElements: PptxElementReference[];
  circuitReferences: CircuitDraftReference[];
  pluginUiContexts: PluginUiModelContext[];
} {
  const plugin = parsePromptPluginUiContexts(stripGenUiModelContext(content));
  const circuit = parsePromptCircuitReferences(plugin.visibleContent);
  const pptx = parsePromptPptxElementReferences(circuit.visibleContent);
  const web = parsePromptWebElementContexts(pptx.visibleContent, workspace);
  const code = parsePromptCodeComments(web.visibleContent, workspace);
  const selections = parsePromptConversationSelections(code.visibleContent);
  return {
    visibleContent: selections.visibleContent,
    codeComments: code.codeCommentAttachments,
    conversationSelections: selections.references,
    webElements: web.webElementContexts,
    pptxElements: pptx.pptxElementReferences,
    circuitReferences: circuit.circuitReferences,
    pluginUiContexts: plugin.pluginUiContexts,
  };
}
