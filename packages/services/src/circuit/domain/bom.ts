/**
 * KiCad 导出的 BOM CSV 解析。
 *
 * 格式依据：
 * - KiCad 自带的 CSV BOM 导出用逗号分隔，含分隔符/引号/换行的字段用双引号包裹，
 *   字段内的双引号用 `""` 转义（RFC 4180）；Excel 在部分区域设置下改成分号分隔。
 *   这里按首行未加引号的分隔符出现次数判断，分号更多时用分号。
 * - Excel 常在文件开头写 UTF-8 BOM，先剥掉，否则首列列名会带上 "\uFEFF" 而识别不到。
 * - Refs 列里的位号可能是 "C1, C2"、"C1;C2" 或 "C1 C2"，只做切分。
 *   **不做** `R1-R3` 区间展开：KiCad 导出的是离散位号列表，区间写法（Altium 等）在 P1 不解析，
 *   原样保留可以避免上层误解出并不存在的器件。
 *
 * 纯函数、零 IO。项目规则：回路里读不到的列一律原样放进 `fields`，不猜语义。
 */

export interface BomDocument {
  rows: BomRow[];
}

export interface BomRow {
  refs: string[];
  quantity?: number;
  value?: string;
  footprint?: string;
  partNumber?: string;
  /** 其它列原样保留：列名 → 值 */
  fields: Record<string, string>;
}

type BomColumnRole = "refs" | "quantity" | "value" | "footprint" | "partNumber";

export function parseBomCsv(text: string): BomDocument {
  const source = stripBom(text);
  const records = parseCsvRecords(source, detectDelimiter(source));
  const header = records[0];
  if (header === undefined) return { rows: [] };
  const roles = header.map(readColumnRole);
  // 一个列名都认不出来时按“没有表头”处理：整份返回空 rows。
  // 决定：宁可返回空，也不把头行当数据、按位置猜列——猜错列会把值错配到 partNumber/footprint 上。
  if (roles.every((role) => role === undefined)) return { rows: [] };
  const rows = records
    .slice(1)
    .filter((record) => record.some((cell) => cell.trim().length > 0))
    .map((record) => buildRow(header, roles, record));
  return { rows };
}

function buildRow(header: string[], roles: (BomColumnRole | undefined)[], record: string[]): BomRow {
  const fields: Record<string, string> = {};
  let refs: string[] = [];
  let quantity: number | undefined;
  let value: string | undefined;
  let footprint: string | undefined;
  let partNumber: string | undefined;
  for (let index = 0; index < header.length; index += 1) {
    const name = header[index] ?? "";
    // 该行比表头短时按空单元格处理；比表头长的多余额外单元格没有列名可归属，直接丢弃。
    const cell = record[index] ?? "";
    switch (roles[index]) {
      case "refs":
        refs = splitRefs(cell);
        break;
      case "quantity":
        quantity = parseQuantity(cell);
        break;
      case "value":
        value = nonEmpty(cell);
        break;
      case "footprint":
        footprint = nonEmpty(cell);
        break;
      case "partNumber":
        partNumber = nonEmpty(cell);
        break;
      default:
        if (name.trim().length > 0) fields[name] = cell;
        break;
    }
  }
  return { refs, quantity, value, footprint, partNumber, fields };
}

/** 位号按逗号、分号、空白切分并去掉空白项，不做区间展开。 */
function splitRefs(cell: string): string[] {
  return cell.split(/[,;\s]+/).filter((ref) => ref.length > 0);
}

/** Qty 列非数字（如 "2 pcs"）时省略，不猜数量。 */
function parseQuantity(cell: string): number | undefined {
  const trimmed = cell.trim();
  if (trimmed.length === 0) return undefined;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/** 列名大小写与空格不敏感：Reference/References/Refs、Qty/Quantity、MPN/Part Number 都识别。 */
function readColumnRole(header: string): BomColumnRole | undefined {
  switch (header.trim().toLowerCase().replace(/\s+/g, "")) {
    case "ref":
    case "refs":
    case "reference":
    case "references":
      return "refs";
    case "qty":
    case "quantity":
      return "quantity";
    case "value":
      return "value";
    case "footprint":
      return "footprint";
    case "mpn":
    case "partnumber":
      return "partNumber";
    default:
      return undefined;
  }
}

/** 按首行判断分隔符：只数引号外的字符，分号多于逗号时用分号，否则用逗号。 */
function detectDelimiter(text: string): string {
  let commas = 0;
  let semicolons = 0;
  let inQuotes = false;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index]!;
    if (char === '"') {
      // 引号内的 `""` 会连续翻转两次，净效果不变，因此不必区分转义。
      inQuotes = !inQuotes;
      continue;
    }
    if (!inQuotes) {
      if (char === "\n") break;
      if (char === ",") commas += 1;
      else if (char === ";") semicolons += 1;
    }
  }
  return semicolons > commas ? ";" : ",";
}

/**
 * 逐字符读取 CSV 记录。支持引号包裹、`""` 转义和字段内换行（CRLF/LF/CR 都算换行）。
 * 引号没有闭合时不抛错，按读到的最长内容产出：损坏的 CSV 由上层按数据问题处理。
 */
function parseCsvRecords(text: string, delimiter: string): string[][] {
  const records: string[][] = [];
  let record: string[] = [];
  let field = "";
  let inQuotes = false;
  let index = 0;
  while (index < text.length) {
    const char = text[index]!;
    if (inQuotes) {
      if (char === '"') {
        if (text[index + 1] === '"') {
          field += '"';
          index += 2;
          continue;
        }
        inQuotes = false;
        index += 1;
        continue;
      }
      field += char;
      index += 1;
      continue;
    }
    if (char === '"' && field.length === 0) {
      inQuotes = true;
      index += 1;
      continue;
    }
    if (char === delimiter) {
      record.push(field);
      field = "";
      index += 1;
      continue;
    }
    if (char === "\r" || char === "\n") {
      record.push(field);
      records.push(record);
      record = [];
      field = "";
      index += char === "\r" && text[index + 1] === "\n" ? 2 : 1;
      continue;
    }
    field += char;
    index += 1;
  }
  // 末尾没有换行时补最后一条记录；只有换行结尾、没有残余内容时不补空行。
  if (field.length > 0 || record.length > 0) {
    record.push(field);
    records.push(record);
  }
  return records;
}

function stripBom(text: string): string {
  return text.startsWith("\uFEFF") ? text.slice(1) : text;
}

function nonEmpty(value: string): string | undefined {
  return value.length > 0 ? value : undefined;
}
