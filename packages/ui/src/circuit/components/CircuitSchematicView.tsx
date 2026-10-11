import { memo } from "react";
import type {
  CircuitLayoutShape,
  CircuitLayoutSheet,
  CircuitLayoutSymbol,
} from "@escode/services";
import { cn } from "@/components/lib/utils.js";
import {
  arcPath,
  highlightedPinPoints,
  isSymbolHighlighted,
  layoutViewBox,
  polylinePoints,
  wireTouchesPoints,
  type CircuitHighlight,
} from "@/circuit/domain/circuitView.js";

/**
 * 原理图 / PCB 的 SVG 渲染（规格 §8「KiCad：根据解析出的几何信息自己渲染 SVG」）。
 * 只吃 `CircuitLayout` 的几何与 IR 侧的选中/低置信度集合；坐标单位是 mm，viewBox 直接用图纸坐标。
 */

const STROKE_WIDTH = 0.16;
const WIRE_STROKE_WIDTH = 0.14;
const PIN_STROKE_WIDTH = 0.12;
const HIGHLIGHT_STROKE_WIDTH = 0.3;
const JUNCTION_RADIUS = 0.5;
const PIN_POINT_RADIUS = 0.22;
const LABEL_FONT_SIZE = 1.3;
const TEXT_FONT_SIZE = 1.35;

function shapePath(shape: CircuitLayoutShape): string {
  if (shape.kind === "rectangle") {
    const [first, second] = shape.points;
    if (!first || !second) return "";
    const x = Math.min(first.x, second.x);
    const y = Math.min(first.y, second.y);
    return `M${x} ${y} h${Math.abs(second.x - first.x)} v${Math.abs(second.y - first.y)} h${-Math.abs(second.x - first.x)} Z`;
  }
  if (shape.kind === "circle") {
    const [center, edge] = shape.points;
    if (!center || !edge) return "";
    const radius = Math.hypot(edge.x - center.x, edge.y - center.y);
    return `M${center.x - radius} ${center.y} a${radius} ${radius} 0 1 0 ${radius * 2} 0 a${radius} ${radius} 0 1 0 ${-radius * 2} 0`;
  }
  if (shape.kind === "arc") return arcPath(shape.points);
  return shape.points.map((point, index) => `${index === 0 ? "M" : "L"}${point.x} ${point.y}`).join(" ");
}

function SymbolShapes({ symbol }: { symbol: CircuitLayoutSymbol }) {
  return (
    <>
      {symbol.shapes.map((shape, index) => (
        <path
          key={`shape:${index}`}
          d={shapePath(shape)}
          className={cn("stroke-foreground", shape.filled ? "fill-foreground/15" : "fill-none")}
          strokeWidth={STROKE_WIDTH}
        />
      ))}
    </>
  );
}

export interface CircuitSchematicViewLabels {
  schematic: string;
  lowConfidence: string;
}

export const CircuitSchematicView = memo(function CircuitSchematicView({
  sheet,
  highlight,
  lowConfidenceIds,
  selectedComponentId,
  onSelectComponent,
  onSelectNetName,
  labels,
}: {
  sheet: CircuitLayoutSheet;
  highlight: CircuitHighlight;
  lowConfidenceIds: ReadonlySet<string>;
  selectedComponentId?: string;
  onSelectComponent?: (componentId: string) => void;
  onSelectNetName?: (name: string) => void;
  labels: CircuitSchematicViewLabels;
}) {
  const viewBox = layoutViewBox(sheet);
  const pinPoints = highlight.pins.size > 0 ? highlightedPinPoints(sheet, highlight) : [];
  return (
    <svg
      data-testid="circuit-schematic"
      data-circuit-sheet-path={sheet.path}
      viewBox={`${viewBox.minX} ${viewBox.minY} ${viewBox.width} ${viewBox.height}`}
      className="h-full w-full text-foreground"
      role="img"
      aria-label={labels.schematic}
    >
      <rect
        x={0}
        y={0}
        width={sheet.page.width}
        height={sheet.page.height}
        className="fill-surface/40 stroke-border"
        strokeWidth={STROKE_WIDTH}
      />
      {sheet.wires.map((wire, index) => {
        const highlighted = wireTouchesPoints(wire, pinPoints);
        return (
          <polyline
            key={`wire:${index}`}
            points={polylinePoints(wire.points)}
            className={cn(
              "fill-none",
              highlighted ? "stroke-brand" : "stroke-foreground/70",
            )}
            strokeWidth={highlighted ? HIGHLIGHT_STROKE_WIDTH : WIRE_STROKE_WIDTH}
          />
        );
      })}
      {sheet.junctions.map((junction, index) => (
        <circle
          key={`junction:${index}`}
          cx={junction.x}
          cy={junction.y}
          r={JUNCTION_RADIUS}
          className="fill-foreground"
        />
      ))}
      {sheet.symbols.map((symbol, index) => {
        const elementId = symbol.elementId;
        const isSelected = elementId !== undefined && elementId === selectedComponentId;
        const isHighlighted = isSymbolHighlighted(symbol, highlight);
        const isLowConfidence = elementId !== undefined && lowConfidenceIds.has(elementId);
        const interactive = elementId !== undefined && onSelectComponent !== undefined;
        return (
          <g
            key={symbol.elementId ?? `symbol:${index}`}
            data-testid={elementId ? "circuit-symbol" : undefined}
            data-circuit-element-id={elementId}
            data-circuit-selected={isSelected ? "true" : undefined}
            data-circuit-low-confidence={isLowConfidence ? "true" : undefined}
            role={interactive ? "button" : undefined}
            tabIndex={interactive ? 0 : undefined}
            className={cn(
              interactive && "cursor-pointer focus-visible:outline-2 focus-visible:outline-brand",
              isLowConfidence && "text-warning",
              isHighlighted && !isLowConfidence && "text-brand",
              isSelected && "text-brand",
            )}
            onClick={elementId && onSelectComponent ? () => onSelectComponent(elementId) : undefined}
            onKeyDown={
              elementId && onSelectComponent
                ? (event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      onSelectComponent(elementId);
                    }
                  }
                : undefined
            }
          >
            {isLowConfidence ? (
              <title>{labels.lowConfidence}</title>
            ) : null}
            <SymbolShapes symbol={symbol} />
            {symbol.pins.map((pin) => {
              const pinHighlighted = highlight.pins.has(
                `${elementId ?? ""}\u0000${pin.number}`,
              );
              return (
                <g key={`pin:${pin.number}`}>
                  <line
                    x1={pin.body.x}
                    y1={pin.body.y}
                    x2={pin.connection.x}
                    y2={pin.connection.y}
                    className={cn(
                      pinHighlighted
                        ? "stroke-brand"
                        : isLowConfidence
                          ? "stroke-warning"
                          : "stroke-foreground/70",
                    )}
                    strokeWidth={pinHighlighted ? HIGHLIGHT_STROKE_WIDTH : PIN_STROKE_WIDTH}
                  />
                  <circle
                    cx={pin.connection.x}
                    cy={pin.connection.y}
                    r={PIN_POINT_RADIUS}
                    className={cn(pinHighlighted ? "fill-brand" : "fill-foreground/70")}
                  />
                </g>
              );
            })}
            {symbol.texts
              .filter((text) => !text.hidden)
              .map((text, textIndex) => (
                <text
                  key={`text:${textIndex}`}
                  x={text.x}
                  y={text.y}
                  fontSize={TEXT_FONT_SIZE}
                  transform={text.rotation ? `rotate(${text.rotation} ${text.x} ${text.y})` : undefined}
                  className="fill-current select-none"
                >
                  {text.text}
                </text>
              ))}
            {symbol.powerName ? (
              <text
                x={symbol.x}
                y={symbol.y + 2.2}
                fontSize={TEXT_FONT_SIZE}
                className="fill-current select-none"
                data-circuit-power={symbol.powerName}
              >
                {symbol.powerName}
              </text>
            ) : null}
          </g>
        );
      })}
      {sheet.labels.map((label, index) => (
        <text
          key={`label:${index}`}
          x={label.x}
          y={label.y}
          fontSize={LABEL_FONT_SIZE}
          transform={label.rotation ? `rotate(${label.rotation} ${label.x} ${label.y})` : undefined}
          className={cn(
            "fill-foreground-subtle select-none",
            onSelectNetName && "cursor-pointer hover:fill-brand",
          )}
          onClick={onSelectNetName ? () => onSelectNetName(label.text) : undefined}
          data-circuit-label-kind={label.kind}
        >
          {label.text}
        </text>
      ))}
    </svg>
  );
});
