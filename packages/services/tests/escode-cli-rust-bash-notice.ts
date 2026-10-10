/**
 * 解析 Bash 后台说明（TS formatBashModelContent 文案，docs/specs/rust-bash-model-content.md）：
 * `Command running in background with ID: <id>. Output is being written to: <path>. …`
 */
export function backgroundNotice(content: string) {
  const match = /with ID: (\S+?)\.(?: Output is being written to: (.+?)\.(?: |$))?/.exec(content);
  if (!match) throw new Error(`Not a Bash background notice: ${content}`);
  return { taskId: match[1]!, outputFile: match[2] };
}
