// Rust 单文件规模门禁的共享口径：有效代码行。
//
// 只统计会产生行为的行（非空、且不在整行注释内），行尾注释仍计入所在代码行。
// 文档注释和空行不占运行时复杂度，按物理行统计会逼着注释密集的模块做机械拆分；
// 这里统一按有效代码行判断，CLI 与 Host 两个 workspace 使用同一口径。
export function countRustCodeLines(source) {
  let inBlockComment = false;
  let lines = 0;
  for (const line of source.split(/\r?\n/)) {
    let index = 0;
    let hasCode = false;
    while (index < line.length) {
      if (inBlockComment) {
        const end = line.indexOf("*/", index);
        if (end === -1) {
          index = line.length;
          break;
        }
        inBlockComment = false;
        index = end + 2;
        continue;
      }
      const rest = line.slice(index);
      if (rest.startsWith("//")) break;
      if (rest.startsWith("/*")) {
        inBlockComment = true;
        index += 2;
        continue;
      }
      if (line[index] !== " " && line[index] !== "\t") hasCode = true;
      index += 1;
    }
    if (hasCode) lines += 1;
  }
  return lines;
}
