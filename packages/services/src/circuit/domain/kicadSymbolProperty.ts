import { atomAt, childListsOf, firstChild, readPoint, type SExprList } from "./sexpr.js";

/**
 * 符号实例的属性节点读取（渲染用）：`properties` 只有名字→值，面板画文本还需要 `(at …)` 坐标与
 * `(effects … hide)` 标记。单位 mm、**图纸坐标**（实例上的 property 直接写在图纸坐标里，不需要变换）。
 * 与 kicadSchematic.ts 分开放是为了守住单文件行数上限，不改变模型语义。
 */

/** 属性的渲染信息：名字、值、文本位置（图纸坐标）与隐藏标记。 */
export interface KicadSymbolProperty {
  name: string;
  value: string;
  at: { x: number; y: number; rotation: number };
  hidden: boolean;
}

/** 逐个读实例的 `(property "Reference" "R1" (at …) …)`；读不出名字/值/坐标的节点直接跳过。 */
export function readPropertyPlacements(node: SExprList): KicadSymbolProperty[] {
  return childListsOf(node, "property").flatMap((property) => {
    const name = atomAt(property, 1);
    const value = atomAt(property, 2);
    const at = readPoint(firstChild(property, "at"));
    if (name === undefined || value === undefined || !at) return [];
    return [
      { name, value, at: { x: at.x, y: at.y, rotation: at.rotation ?? 0 }, hidden: isPropertyHidden(property) },
    ];
  });
}

/** 属性隐藏标记：KiCad 6-8 写作 `(effects (font …) hide)`，KiCad 9 写作 `(effects (hide yes))`。 */
function isPropertyHidden(property: SExprList): boolean {
  const effects = firstChild(property, "effects");
  if (!effects) return false;
  const hideNode = firstChild(effects, "hide");
  if (hideNode && atomAt(hideNode, 1) !== "no") return true;
  return effects.items.some((item) => item.kind === "atom" && item.value === "hide");
}
