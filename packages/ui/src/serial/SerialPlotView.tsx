import { useEffect, useMemo, useRef, useState } from "react";
import { CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";
import type { SerialChunk } from "@escode/services";
import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { SerialDisplayEncoding } from "@/lib/serial/serialFormat.js";
import { SerialPlotBuffer, type SerialPlotRow } from "@/lib/serial/serialPlot.js";

/** 重绘节流：最多每秒 10 次。 */
const REDRAW_INTERVAL_MS = 100;

const SERIES_COLORS = [
  "var(--color-usage-chart-1)",
  "var(--color-usage-chart-2)",
  "var(--color-usage-chart-3)",
  "var(--color-usage-chart-4)",
  "var(--color-usage-chart-5)",
  "var(--color-usage-chart-6)",
] as const;

interface PlotSnapshot {
  series: string[];
  rows: SerialPlotRow[];
}

/**
 * 波形视图（docs/specs/serial-port-debugger-phase3.md 第 6 节）：解析在缓冲里增量进行，
 * 渲染快照按节流间隔刷新。暂停由日志视图冻结 chunks 实现，恢复后增量补齐。
 */
export function SerialPlotView({
  chunks,
  encoding,
}: {
  chunks: readonly SerialChunk[];
  encoding: SerialDisplayEncoding;
}) {
  const { intl } = useESCodeIntl();
  const buffer = useMemo(() => new SerialPlotBuffer(encoding), [encoding]);
  const [snapshot, setSnapshot] = useState<PlotSnapshot>({ series: [], rows: [] });
  const lastFlush = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!buffer.push(chunks) || timer.current) return;
    const flush = () => {
      timer.current = null;
      lastFlush.current = Date.now();
      setSnapshot({ series: [...buffer.series], rows: buffer.rows.map((row) => ({ ...row })) });
    };
    const wait = Math.max(0, REDRAW_INTERVAL_MS - (Date.now() - lastFlush.current));
    timer.current = setTimeout(flush, wait);
  }, [buffer, chunks]);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
      timer.current = null;
    },
    [buffer],
  );

  const config = useMemo<ChartConfig>(
    () => Object.fromEntries(snapshot.series.map((name) => [name, { label: name }])),
    [snapshot.series],
  );

  if (snapshot.rows.length === 0) {
    return (
      <div
        className="flex min-h-0 flex-1 items-center justify-center rounded-xl border border-border bg-surface p-4 text-center text-ui-sm text-foreground-subtle"
        data-testid="serial-plot-empty"
      >
        {intl.formatMessage({ id: "serial.plot.empty" })}
      </div>
    );
  }

  return (
    <div
      className="min-h-0 flex-1 rounded-xl border border-border bg-surface p-2"
      data-testid="serial-plot"
      data-series={snapshot.series.join(",")}
      data-points={snapshot.rows.length}
    >
      <ChartContainer config={config} className="aspect-auto h-full w-full">
        <LineChart data={snapshot.rows} margin={{ top: 8, right: 16, left: 0, bottom: 0 }}>
          <CartesianGrid vertical={false} />
          <XAxis
            dataKey="t"
            type="number"
            domain={["dataMin", "dataMax"]}
            tickFormatter={(value: number) => `${value.toFixed(1)}s`}
            tickLine={false}
            axisLine={false}
          />
          <YAxis domain={["auto", "auto"]} width={48} tickLine={false} axisLine={false} />
          <ChartTooltip
            isAnimationActive={false}
            content={
              <ChartTooltipContent
                labelFormatter={(_, payload) => {
                  const t = (payload?.[0]?.payload as SerialPlotRow | undefined)?.t;
                  return t === undefined ? "" : `${t.toFixed(3)}s`;
                }}
              />
            }
          />
          <ChartLegend content={<ChartLegendContent />} />
          {snapshot.series.map((name, index) => (
            <Line
              key={name}
              dataKey={name}
              type="linear"
              stroke={SERIES_COLORS[index % SERIES_COLORS.length]}
              // 颜色只有 6 种，第 7、8 条曲线用虚线区分。
              strokeDasharray={index >= SERIES_COLORS.length ? "4 3" : undefined}
              strokeWidth={1.5}
              dot={false}
              connectNulls
              isAnimationActive={false}
            />
          ))}
        </LineChart>
      </ChartContainer>
    </div>
  );
}
