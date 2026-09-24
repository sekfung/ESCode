//! 差分：Rust permission::check 与 TS PermissionService 的判定矩阵逐条一致。
//! 产物由 scripts/generate-zcode-cli-rust-permission-matrix.mjs 生成，枚举顺序以其注释为准。
use serde_json::Value;
use zcode_cli_domain::permission::{Capability, Context, Mode, Risk, check};

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

fn risk(s: &str) -> Risk {
    match s {
        "low" => Risk::Low,
        "medium" => Risk::Medium,
        "high" => Risk::High,
        "critical" => Risk::Critical,
        _ => panic!("risk {s}"),
    }
}

#[test]
fn rust_matches_ts_permission_matrix() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/permission_matrix.json")).unwrap();
    let axes = &fixture["axes"];
    let outcomes = strings(&fixture["outcomes"]);
    let expected: Vec<&str> = fixture["decisions"]
        .as_str()
        .unwrap()
        .chars()
        .map(|c| outcomes[(c as u32 - 48) as usize].as_str())
        .collect();
    let modes: Vec<(Mode, Option<bool>)> = axes["modes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (Mode::parse(m[0].as_str().unwrap()).unwrap(), m[1].as_bool()))
        .collect();
    let flags = strings(&axes["flags"]);
    let mut caps = Vec::new();
    for bits in 0..(1u32 << flags.len()) {
        for scope in strings(&axes["scopes"]) {
            for r in strings(&axes["risks"]) {
                for name in axes["permissionNames"].as_array().unwrap() {
                    let on = |flag: &str| {
                        let i = flags.iter().position(|f| f == flag).unwrap();
                        Some(bits & (1 << i) != 0)
                    };
                    caps.push(Capability {
                        read_only: on("readOnly"),
                        destructive: on("destructive"),
                        always_ask: on("alwaysAsk"),
                        requires_user_interaction: on("requiresUserInteraction"),
                        needs_approval: on("needsApproval"),
                        allowed_in_plan_mode: on("allowedInPlanMode"),
                        side_effect_scope: Some(scope.clone()),
                        risk_level: Some(risk(&r)),
                        permission_name: name.as_str().map(str::to_owned),
                    });
                }
            }
        }
    }
    let mut cases: Vec<(String, Mode, Option<bool>, Option<&Capability>)> = Vec::new();
    for tool in strings(&axes["toolsWithDefaults"]) {
        for (mode, plan) in &modes {
            cases.push((tool.clone(), *mode, *plan, None));
        }
    }
    for tool in strings(&axes["toolsWithCapability"]) {
        for (mode, plan) in &modes {
            for cap in &caps {
                cases.push((tool.clone(), *mode, *plan, Some(cap)));
            }
        }
    }
    assert_eq!(cases.len(), expected.len(), "matrix size");
    let mismatches: Vec<String> = cases
        .iter()
        .zip(&expected)
        .filter_map(|((tool, mode, plan, cap), want)| {
            let got = check(
                &Context {
                    tool_name: tool,
                    mode: *mode,
                    plan_enabled: *plan,
                },
                *cap,
            );
            let got = format!("{}:{}", got.behavior.as_str(), got.rule_id);
            (got != *want).then(|| format!("{tool} {mode:?} {plan:?} {cap:?}: {got} != {want}"))
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "{} mismatches, first: {:#?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(5)]
    );
}
