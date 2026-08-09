//! Regression contract for packaged observability artifacts.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn observability_root() -> PathBuf {
    repo_root().join("deploy/observability")
}

fn grafana_dashboard_paths() -> Vec<PathBuf> {
    let mut paths = fs::read_dir(observability_root().join("grafana"))
        .expect("grafana directory")
        .map(|entry| entry.expect("dashboard dir entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn flatten_panels<'a>(panels: &'a [JsonValue], output: &mut Vec<&'a JsonValue>) {
    for panel in panels {
        output.push(panel);
        if let Some(children) = panel.get("panels").and_then(JsonValue::as_array) {
            flatten_panels(children, output);
        }
    }
}

#[test]
fn grafana_dashboards_parse_and_expose_required_contract_fields() {
    let dashboards = grafana_dashboard_paths();
    assert_eq!(
        dashboards.len(),
        6,
        "expected the packaged observability bundle to ship six Grafana dashboards"
    );

    for dashboard in dashboards {
        let source = fs::read_to_string(&dashboard).expect("read dashboard");
        let value: JsonValue =
            serde_json::from_str(&source).unwrap_or_else(|err| panic!("{dashboard:?}: {err}"));

        assert!(
            value.get("uid").and_then(JsonValue::as_str).is_some(),
            "{dashboard:?} must declare a stable uid"
        );
        assert!(
            value.get("title").and_then(JsonValue::as_str).is_some(),
            "{dashboard:?} must declare a human title"
        );
        assert!(
            value.get("schemaVersion").and_then(JsonValue::as_u64).is_some(),
            "{dashboard:?} must declare a Grafana schemaVersion"
        );
        assert!(
            value.get("version").and_then(JsonValue::as_u64).is_some(),
            "{dashboard:?} must declare an artifact version"
        );

        let panels = value["panels"]
            .as_array()
            .unwrap_or_else(|| panic!("{dashboard:?} must contain a top-level panels array"));
        assert!(
            !panels.is_empty(),
            "{dashboard:?} must contain at least one panel"
        );
        assert!(
            value["annotations"]["list"].as_array().is_some(),
            "{dashboard:?} must declare annotation configuration"
        );
        let templating = value["templating"]["list"]
            .as_array()
            .unwrap_or_else(|| panic!("{dashboard:?} must declare dashboard variables"));
        assert!(
            templating.iter().any(|entry| {
                entry["name"].as_str() == Some("datasource")
                    && entry["type"].as_str() == Some("datasource")
            }),
            "{dashboard:?} must include a datasource variable"
        );

        let mut flattened = Vec::new();
        flatten_panels(panels, &mut flattened);
        for panel in flattened {
            assert!(
                panel.get("id").and_then(JsonValue::as_u64).is_some(),
                "{dashboard:?} contains a panel without an id"
            );
            assert!(
                panel.get("type").and_then(JsonValue::as_str).is_some(),
                "{dashboard:?} contains a panel without a type"
            );
            if let Some(targets) = panel.get("targets").and_then(JsonValue::as_array) {
                for target in targets {
                    assert!(
                        target.get("refId").and_then(JsonValue::as_str).is_some(),
                        "{dashboard:?} contains a query target without a refId"
                    );
                    if let Some(expr) = target.get("expr").and_then(JsonValue::as_str) {
                        assert!(
                            !expr.trim().is_empty(),
                            "{dashboard:?} contains an empty PromQL expression"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn prometheus_rule_files_parse_and_expose_required_contract_fields() {
    for relative in [
        "prometheus/recording-rules.yaml",
        "prometheus/alerts.yaml",
    ] {
        let path = observability_root().join(relative);
        let source = fs::read_to_string(&path).expect("read Prometheus rules file");
        let value: YamlValue =
            serde_yaml::from_str(&source).unwrap_or_else(|err| panic!("{path:?}: {err}"));

        let groups = value["groups"]
            .as_sequence()
            .unwrap_or_else(|| panic!("{path:?} must contain a groups sequence"));
        assert!(
            !groups.is_empty(),
            "{path:?} must contain at least one rule group"
        );

        for group in groups {
            assert!(
                group["name"].as_str().is_some(),
                "{path:?} contains a rule group without a name"
            );
            let rules = group["rules"]
                .as_sequence()
                .unwrap_or_else(|| panic!("{path:?} contains a group without rules"));
            assert!(
                !rules.is_empty(),
                "{path:?} contains an empty rule group"
            );

            for rule in rules {
                assert!(
                    rule["expr"].as_str().is_some(),
                    "{path:?} contains a rule without an expr"
                );
                assert!(
                    rule["record"].as_str().is_some() || rule["alert"].as_str().is_some(),
                    "{path:?} contains a rule without a record or alert name"
                );
            }
        }
    }
}
