//! Agent loadout profiles (#44) — exact copy of _AGENT_PROFILES.

use serde_json::{json, Value};

pub struct Profile {
    pub description: &'static str,
    pub domains: &'static [&'static str],
}

pub const PROFILES: &[(&str, Profile)] = &[
    (
        "dev-1c",
        Profile {
            description: "1C/BSL developer: code fixes, pitfalls, farm deploys",
            domains: &[
                "pitfalls", "deploy", "deployment", "infrastructure", "tasks",
                "validation", "testing", "testing-ui", "implementation",
                "integrations", "1c-ai-testing", "investigation", "reference", "spec",
            ],
        },
    ),
    (
        "em",
        Profile {
            description: "Engineering manager: projects, decisions, current work",
            domains: &[
                "work_map", "work_current", "work_context", "project",
                "project_status", "process", "retrospective", "reviews",
                "feedback", "team", "tasks", "user",
            ],
        },
    ),
    (
        "infra",
        Profile {
            description: "Infra/ops: servers, network, security, licensing",
            domains: &["infrastructure", "security", "security-audit", "licensing", "pitfalls"],
        },
    ),
    (
        "default",
        Profile {
            description: "No domain bias (previous behavior)",
            domains: &[],
        },
    ),
];

pub fn get(name: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|(n, _)| *n == name).map(|(_, p)| p)
}

pub fn list_response() -> Value {
    let mut profiles = serde_json::Map::new();
    for (name, p) in PROFILES {
        profiles.insert(
            name.to_string(),
            json!({"description": p.description, "domains": p.domains}),
        );
    }
    json!({"status": "success", "profiles": Value::Object(profiles)})
}