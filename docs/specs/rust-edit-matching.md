# Rust Edit 宽松匹配（对齐 TS `edit-matchers.ts`）

2026-09-25。此前 Rust `Edit` 只做精确子串匹配，模型传入的 `old_string` 与文件有弯引号、`Read` 行号前缀、
转义字符或缩进差异时直接失败，而 Node runtime 能匹配成功——同一轮对话在两个 runtime 上结果不同。

## 规则（唯一事实源：`apps/zcode-cli/packages/core/src/tool/edit-matchers.ts`）

- 文件内容、`old_string`、`new_string` 先统一 `\r\n → \n`（现状不变）。
- 精确匹配优先；没有精确候选时按顺序尝试，**第一个有候选的策略决定结果**：
  `quote_normalized` → `line_number_prefix_stripped` → `escape_normalized` → `unicode_escape_normalized`
  → `line_trimmed` → `indentation_flexible` → `block_anchor`。
- `replace_all=true` 时跳过三个宽泛策略（`line_trimmed`、`indentation_flexible`、`block_anchor`）。
- 候选去重后多于一种文本 → 歧义（`candidateCount` 为候选数）；唯一文本 → 以文件中的真实片段为 `oldString`。
  之后再按真实片段在文件中计数，非 `replace_all` 且出现多次同样判歧义。
- `escape_normalized` 命中时 `new_string` 同样反转义；弯引号命中时 `new_string` 中的直引号按上下文转为弯引号。
- `new_string` 为空且 `old_string` 不以换行结尾、文件中存在 `old_string + "\n"` 时，一并删除该换行。
- 相似度（`block_anchor` 中间行平均 ≥ 0.8）按 UTF-16 码元计算 Levenshtein，与 TS 字符串长度语义一致。
- 结果字段：`matchStrategy`、`matchCandidateCount` 为实际策略与候选数；`old_string` 为空（新建/空文件）时两者缺省。
- 错误文案与 TS 一致：未找到 `String to replace not found in file.\nString: <old_string>`；
  歧义 `Found N matches of the string to replace, but replace_all is false. …`（N=0 时用 TS 的 non-unique 文案）。

## 验收

- Rust 单测覆盖每个策略、`replace_all` 跳过宽泛策略、歧义与引号风格保留。
- `zcode-cli-rust-tool-parity.test.ts`：同一文件与参数分别交给 TS `editToolEntry` 与 Rust `Edit`，
  比对写入后的文件内容与结果字段（不含路径/patch），覆盖全部策略与失败场景。
