// Run with node --import tsx. 用 TS selectActiveConversationBranch 导出回退活跃分支语料，
// Rust domain::rewind_branch 逐条对照（--check 防漂移）。见 docs/specs/rust-import-lifecycle.md。
import { readFile, writeFile } from "node:fs/promises";
import { selectActiveConversationBranch } from "../apps/zcode-cli/packages/contracts/src/rewind/index.ts";

const ids = ["m0", "m1", "m2", "m3", "m4", "m5"];
const pick = [undefined, "m0", "m2", "m5", "missing"];
const keptSets = [undefined, [], ["m0"], ["m0", "m1"], ["m3", "m1"], ["m0", "missing"]];
const cases = [];
for (const target of pick)
  for (const kept of keptSets)
    for (const cut of pick)
      for (const created of pick) {
        const revert = {
          ...(target ? { targetMessageID: target } : {}),
          ...(kept ? { keptMessageIDs: kept } : {}),
          ...(cut ? { branchCutAfterMessageID: cut } : {}),
          ...(created ? { createdMessageID: created } : {}),
        };
        const active = selectActiveConversationBranch(
          ids.map((id) => ({ info: { id } })),
          {
            rewindTargetMessageId: target,
            rewindKeptMessageIds: kept,
            branchCutAfterMessageId: cut,
            rewindCreatedMessageId: created,
          },
        ).map((m) => ids.indexOf(m.info.id));
        cases.push([revert, active]);
      }

const content = `${JSON.stringify({ ids, cases })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/rewind_branch_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust rewind branch corpus differs from TS");
} else await writeFile(target, content);
