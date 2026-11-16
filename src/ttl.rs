//! TTL processing — port of TTLManager.process_expired (v2 ttl.py).

use serde_json::{json, Value};

use crate::store::{Store, TtlConfig};

pub fn default_ttl_for_level(level: i64) -> TtlConfig {
    match level {
        0 => TtlConfig { ttl_seconds: 30 * 24 * 3600, refresh_trigger: None, on_expire: "mark_stale".into() },
        1 => TtlConfig { ttl_seconds: 7 * 24 * 3600, refresh_trigger: Some("**/api/**/*.py".into()), on_expire: "mark_stale".into() },
        2 => TtlConfig { ttl_seconds: 3 * 24 * 3600, refresh_trigger: None, on_expire: "archive".into() },
        3 => TtlConfig { ttl_seconds: 24 * 3600, refresh_trigger: None, on_expire: "delete".into() },
        _ => TtlConfig { ttl_seconds: 3 * 24 * 3600, refresh_trigger: None, on_expire: "archive".into() },
    }
}

pub fn process_expired(store: &Store) -> Value {
    let mut processed = 0i64;
    let mut marked_stale = 0i64;
    let mut archived = 0i64;
    let mut deleted = 0i64;
    let mut errors: Vec<String> = Vec::new();

    let facts = match store.get_all_facts(true, false) {
        Ok(f) => f,
        Err(e) => {
            return json!({"processed": 0, "marked_stale": 0, "archived": 0, "deleted": 0, "refreshed": 0, "errors": [e.to_string()]});
        }
    };

    for fact in &facts {
        if !fact.is_expired() {
            continue;
        }
        processed += 1;
        let cfg = fact.ttl_config.clone().unwrap_or_else(|| default_ttl_for_level(fact.level));
        let result = match cfg.on_expire.as_str() {
            "mark_stale" => {
                if !fact.is_stale {
                    store.mark_stale(&fact.id).map(|ok| {
                        if ok {
                            marked_stale += 1;
                        }
                    })
                } else {
                    Ok(())
                }
            }
            "archive" => store.archive_fact(&fact.id).map(|_| {
                archived += 1;
            }),
            "delete" => store.delete_fact(&fact.id).map(|_| {
                deleted += 1;
            }),
            _ => Ok(()),
        };
        if let Err(e) = result {
            errors.push(format!("Error processing {}: {}", fact.id, e));
        }
    }

    json!({
        "processed": processed,
        "marked_stale": marked_stale,
        "archived": archived,
        "deleted": deleted,
        "refreshed": 0,
        "errors": errors,
    })
}