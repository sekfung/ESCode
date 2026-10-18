export const VERIFIED_DOCKER_TABLE_END_MARKER = "<!-- conversation-session-verified-table-end -->";

export function insertVerifiedDockerSpecRow(text, specRelFromDesktop) {
  const row = `| \`${specRelFromDesktop}\` | verified |`;
  if (text.includes(row) || text.includes(`| \`${specRelFromDesktop}\` |`)) {
    return { inserted: false, text };
  }
  if (!text.includes(VERIFIED_DOCKER_TABLE_END_MARKER)) {
    return {
      error: "Cannot find Docker suite table end marker",
      inserted: false,
      text,
    };
  }

  // 修复：oxfmt 会在表格与 HTML 注释之间保留一个空行（4cb7b20d 全仓格式化后即为此形态）。
  // 行必须接在表格最后一行之后、空行之前，否则会落在表格外；插入后仍保持「表格、空行、标记」的格式化形态。
  const markerIndex = text.indexOf(VERIFIED_DOCKER_TABLE_END_MARKER);
  const tableEnd = text.slice(0, markerIndex).trimEnd();
  return {
    inserted: true,
    text: `${tableEnd}\n${row}\n\n${text.slice(markerIndex)}`,
  };
}
