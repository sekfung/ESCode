/**
 * svn `--xml` 输出的最小解析器。svn 输出只用元素、属性、文本和标准实体，
 * 不含 DTD / CDATA / 命名空间，这里不引入 XML 依赖，只实现这一子集。
 */
export interface SvnXmlNode {
  name: string;
  attrs: Record<string, string>;
  children: SvnXmlNode[];
  text: string;
}

const ENTITY_MAP: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
};

function decodeEntities(value: string): string {
  return value.replace(/&(#x[0-9a-fA-F]+|#\d+|[a-zA-Z]+);/g, (match, entity: string) => {
    if (entity.startsWith("#x")) {
      return String.fromCodePoint(Number.parseInt(entity.slice(2), 16));
    }
    if (entity.startsWith("#")) {
      return String.fromCodePoint(Number.parseInt(entity.slice(1), 10));
    }
    return ENTITY_MAP[entity] ?? match;
  });
}

const ATTR_PATTERN = /([^\s=]+)\s*=\s*(?:"([^"]*)"|'([^']*)')/g;

function parseAttributes(source: string): Record<string, string> {
  const attrs: Record<string, string> = {};
  for (const match of source.matchAll(ATTR_PATTERN)) {
    attrs[match[1]!] = decodeEntities(match[2] ?? match[3] ?? "");
  }
  return attrs;
}

export function parseSvnXml(xml: string): SvnXmlNode | undefined {
  const root: SvnXmlNode = { name: "#root", attrs: {}, children: [], text: "" };
  const stack: SvnXmlNode[] = [root];
  const tagPattern = /<(\/?)([^\s/>?!]+)([^>]*?)(\/?)>|<\?[\s\S]*?\?>|<!--[\s\S]*?-->/g;
  let lastIndex = 0;

  for (const match of xml.matchAll(tagPattern)) {
    const current = stack[stack.length - 1]!;
    const index = match.index ?? 0;
    if (index > lastIndex) {
      current.text += decodeEntities(xml.slice(lastIndex, index));
    }
    lastIndex = index + match[0].length;

    const [, closing, name, attrSource, selfClosing] = match;
    if (!name) {
      continue; // 声明或注释
    }
    if (closing) {
      if (stack.length > 1 && current.name === name) {
        stack.pop();
      }
      continue;
    }
    const node: SvnXmlNode = {
      name,
      attrs: parseAttributes(attrSource ?? ""),
      children: [],
      text: "",
    };
    current.children.push(node);
    if (!selfClosing) {
      stack.push(node);
    }
  }

  return root.children[0];
}

export function childOf(node: SvnXmlNode | undefined, name: string): SvnXmlNode | undefined {
  return node?.children.find((child) => child.name === name);
}

export function childrenOf(node: SvnXmlNode | undefined, name: string): SvnXmlNode[] {
  return node?.children.filter((child) => child.name === name) ?? [];
}

/** 按路径逐级取第一个匹配子节点。 */
export function pathOf(node: SvnXmlNode | undefined, ...names: string[]): SvnXmlNode | undefined {
  let current = node;
  for (const name of names) {
    current = childOf(current, name);
  }
  return current;
}

export function textOf(node: SvnXmlNode | undefined): string | undefined {
  return node ? node.text : undefined;
}

export function intAttr(node: SvnXmlNode | undefined, name: string): number | undefined {
  const raw = node?.attrs[name];
  if (raw === undefined) {
    return undefined;
  }
  const value = Number.parseInt(raw, 10);
  return Number.isFinite(value) ? value : undefined;
}
