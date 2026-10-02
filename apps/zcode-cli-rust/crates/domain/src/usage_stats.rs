//! `v4/usage/stats` 的快照构造（TS bootstrap/zcode-protocol/usage-stats-builder.ts `buildAppUsageSnapshot`）：
//! 由存储层的原始聚合（queryAppUsage 形）算出缓存命中率、活跃 / 连续天数、热力图与按日模型趋势。
use serde_json::{Value, json};
use std::collections::BTreeMap;

const DAY_MS: i64 = 86_400_000;

pub struct Options<'a> {
    pub range: &'a str,
    pub time_zone: &'a str,
    pub tz_offset_ms: i64,
    pub generated_at: i64,
    pub since: i64,
    pub until: i64,
}

/// `dayIndex * DAY` 是「本地午夜当作 UTC」的时刻，取其 UTC 日历分量即本地日期（YYYY-MM-DD）。
fn date(day_index: i64) -> String {
    // Howard Hinnant 的 civil_from_days。
    let z = day_index + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn level(tokens: i64, max: i64) -> i64 {
    if tokens <= 0 || max <= 0 {
        return 0;
    }
    let ratio = tokens as f64 / max as f64;
    if ratio > 0.75 {
        4
    } else if ratio > 0.5 {
        3
    } else if ratio > 0.25 {
        2
    } else {
        1
    }
}

fn int(value: &Value) -> i64 {
    value.as_i64().unwrap_or(0)
}

fn ratio(a: i64, b: i64) -> f64 {
    if b > 0 { a as f64 / b as f64 } else { 0.0 }
}

pub fn snapshot(result: &Value, opts: &Options<'_>) -> Value {
    let (totals, turn_totals, tool_totals) = (
        &result["totals"],
        &result["turnTotals"],
        &result["toolTotals"],
    );
    let input = int(&totals["inputTokens"]);
    let creation = int(&totals["cacheCreationTokens"]);
    let read = int(&totals["cacheReadTokens"]);
    // 用量库的 inputTokens 已是 total input，cache 字段只是 breakdown。
    let cache_denom = if input > 0 { input } else { creation + read };
    let empty = Vec::new();
    let days = result["days"].as_array().unwrap_or(&empty);
    let day_models = result["dayModels"].as_array().unwrap_or(&empty);
    let by_day: BTreeMap<i64, &Value> = days.iter().map(|d| (int(&d["dayIndex"]), d)).collect();
    let end = (opts.until + opts.tz_offset_ms).div_euclid(DAY_MS);
    let start = if opts.range != "all" {
        (opts.since + opts.tz_offset_ms).div_euclid(DAY_MS)
    } else {
        days.iter()
            .chain(day_models)
            .map(|d| int(&d["dayIndex"]))
            .min()
            .unwrap_or(end)
    };
    let tokens_on = |day: i64| by_day.get(&day).map_or(0, |d| int(&d["totalTokens"]));
    let (mut active, mut current, mut longest, mut running, mut broken) = (0, 0, 0, 0, false);
    let mut day = end;
    while day >= start {
        if tokens_on(day) > 0 {
            active += 1;
            running += 1;
            longest = longest.max(running);
            if !broken {
                current += 1;
            }
        } else {
            broken = true;
            running = 0;
        }
        day -= 1;
    }
    let max_tokens = days
        .iter()
        .map(|d| int(&d["totalTokens"]))
        .max()
        .unwrap_or(0)
        .max(0);
    let mut weeks: Vec<Value> = vec![];
    let mut week: Vec<Value> = vec![];
    for day in start..=end {
        let entry = by_day.get(&day);
        let field = |key: &str| entry.map_or(0, |d| int(&d[key]));
        week.push(json!({
            "date": date(day), "level": level(field("totalTokens"), max_tokens),
            "totalTokens": field("totalTokens"), "turnCount": field("turnCount"), "toolCallCount": field("toolCallCount"),
        }));
        if week.len() == 7 {
            weeks.push(json!({ "weekIndex": weeks.len(), "days": std::mem::take(&mut week) }));
        }
    }
    if !week.is_empty() {
        week.resize(7, Value::Null);
        weeks.push(json!({ "weekIndex": weeks.len(), "days": week }));
    }
    // 按日聚合 dayModels（保持模型首次出现的顺序，同 JS Map）。
    let mut daily: BTreeMap<i64, Vec<(Value, i64)>> = BTreeMap::new();
    for dm in day_models {
        let models = daily.entry(int(&dm["dayIndex"])).or_default();
        match models.iter_mut().find(|(id, _)| *id == dm["modelId"]) {
            Some((_, total)) => *total += int(&dm["totalTokens"]),
            None => models.push((dm["modelId"].clone(), int(&dm["totalTokens"]))),
        }
    }
    let daily_model_usage: Vec<Value> = (start..=end)
        .map(|day| {
            let models: Vec<Value> = daily.get(&day).map_or(vec![], |m| {
                m.iter()
                    .map(|(id, total)| json!({ "modelId": id, "totalTokens": total }))
                    .collect()
            });
            json!({ "date": date(day), "models": models })
        })
        .collect();
    let model_rows = result["models"].as_array().unwrap_or(&empty);
    let model_total: i64 = model_rows.iter().map(|m| int(&m["totalTokens"])).sum();
    let models: Vec<Value> = model_rows
        .iter()
        .map(|m| {
            json!({
                "modelId": m["modelId"], "totalTokens": m["totalTokens"], "inputTokens": m["inputTokens"],
                "outputTokens": m["outputTokens"], "requestCount": m["requestCount"],
                "share": ratio(int(&m["totalTokens"]), model_total),
            })
        })
        .collect();
    let favorite = models.first().map_or(Value::Null, |m| {
        json!({ "modelId": m["modelId"], "totalTokens": m["totalTokens"], "share": m["share"] })
    });
    let tools: Vec<Value> = result["tools"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|t| {
            json!({
                "toolName": t["toolName"], "callCount": t["callCount"], "errorCount": t["errorCount"],
                "errorRate": ratio(int(&t["errorCount"]), int(&t["callCount"])), "avgDurationMs": t["avgDurationMs"],
            })
        })
        .collect();
    json!({
        "range": opts.range, "generatedAt": opts.generated_at, "timeZone": opts.time_zone, "source": "agent-db",
        "summary": {
            "totalTokens": totals["totalTokens"], "inputTokens": totals["inputTokens"],
            "outputTokens": totals["outputTokens"], "reasoningTokens": totals["reasoningTokens"],
            "cacheCreationTokens": totals["cacheCreationTokens"], "cacheReadTokens": totals["cacheReadTokens"],
            "cacheHitRate": ratio(read, cache_denom),
            "totalSessions": turn_totals["totalSessions"], "totalTurns": turn_totals["totalTurns"],
            "toolCallCount": tool_totals["toolCallCount"],
            "toolErrorRate": ratio(int(&tool_totals["toolErrorCount"]), int(&tool_totals["toolCallCount"])),
            "modelErrorRate": ratio(int(&totals["modelErrorCount"]), int(&totals["modelRequestCount"])),
            "avgTimeToFirstTokenMs": totals["avgTimeToFirstTokenMs"],
            "avgTurnDurationMs": turn_totals["avgTurnDurationMs"],
            "activeDays": active, "currentStreakDays": current,
            "longestSessionMs": turn_totals["longestSessionMs"], "longestStreakDays": longest,
            "peakDayTokens": max_tokens, "favoriteModel": favorite,
        },
        "heatmap": { "startDate": date(start), "endDate": date(end), "maxTokens": max_tokens, "weeks": weeks },
        "dailyModelUsage": daily_model_usage,
        "models": models,
        "tools": tools,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_follow_the_civil_calendar() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(20_728), "2026-10-02");
    }

    #[test]
    fn streaks_count_back_from_today() {
        let result = json!({"totals":{},"turnTotals":{},"toolTotals":{},"models":[],"tools":[],"dayModels":[],
            "days":[{"dayIndex":10,"totalTokens":5},{"dayIndex":9,"totalTokens":5},{"dayIndex":7,"totalTokens":1}]});
        let opts = Options {
            range: "7d",
            time_zone: "UTC",
            tz_offset_ms: 0,
            generated_at: 0,
            since: 4 * DAY_MS,
            until: 10 * DAY_MS + 5,
        };
        let snapshot = snapshot(&result, &opts);
        assert_eq!(snapshot["summary"]["activeDays"], 3);
        assert_eq!(snapshot["summary"]["currentStreakDays"], 2);
        assert_eq!(snapshot["summary"]["longestStreakDays"], 2);
        assert_eq!(
            snapshot["heatmap"]["weeks"][0]["days"]
                .as_array()
                .unwrap()
                .len(),
            7
        );
    }
}
