//! Fact consolidation — port of FactConsolidator (v2 consolidator.py).

use std::collections::HashMap;

use anyhow::Result;
use serde_json::{json, Value};

use crate::store::{Fact, Store};
use crate::util::{char_slice, jaccard};

pub fn consolidate(store: &Store, min_facts: usize) -> Result<Value> {
    let mut merged_count = 0i64;
    let mut promoted_count = 0i64;
    let mut new_summaries: Vec<String> = Vec::new();

    // L3 -> L2 by module
    let (m, p, s) = consolidate_level(store, 3, 2, "module", min_facts)?;
    merged_count += m;
    promoted_count += p;
    new_summaries.extend(s);

    // L2 -> L1 by domain
    let (m, p, s) = consolidate_level(store, 2, 1, "domain", min_facts)?;
    merged_count += m;
    promoted_count += p;
    new_summaries.extend(s);

    // Dedup within L1 and L2
    let mut archived_count = 0i64;
    for level in [1, 2] {
        let facts = store.get_facts_by_level(level, None, false, false)?;
        let dups = find_duplicates(&facts, 0.8);
        for id in dups {
            store.archive_fact(&id)?;
            archived_count += 1;
        }
    }

    Ok(json!({
        "status": "success",
        "merged_count": merged_count,
        "promoted_count": promoted_count,
        "archived_count": archived_count,
        "new_summaries": new_summaries,
    }))
}

fn consolidate_level(
    store: &Store,
    source_level: i64,
    target_level: i64,
    group_by: &str,
    min_facts: usize,
) -> Result<(i64, i64, Vec<String>)> {
    let mut merged = 0i64;
    let mut promoted = 0i64;
    let mut summaries = Vec::new();

    let facts = store.get_facts_by_level(source_level, None, false, false)?;
    if facts.len() < min_facts {
        return Ok((0, 0, summaries));
    }

    // group preserving first-seen order (Python dict insertion order)
    let mut groups: Vec<(String, Vec<Fact>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for fact in facts {
        let key = match group_by {
            "module" => fact.module.clone(),
            _ => fact.domain.clone(),
        }
        .unwrap_or_else(|| "unknown".to_string());
        let idx = match index.get(&key) {
            Some(i) => *i,
            None => {
                index.insert(key.clone(), groups.len());
                groups.push((key.clone(), Vec::new()));
                groups.len() - 1
            }
        };
        groups[idx].1.push(fact);
    }

    for (key, group) in groups {
        if group.len() >= min_facts {
            // summary: count + up to 3 unique content[:50] topics
            let mut topics: Vec<String> = Vec::new();
            for f in &group {
                let t = char_slice(&f.content, 0, 50);
                if !topics.contains(&t) {
                    topics.push(t);
                }
                if topics.len() == 3 {
                    break;
                }
            }
            let summary = format!("[{}] {} related facts about: {}", key, group.len(), topics.join("; "));
            let module = if target_level == 2 { Some(key.as_str()) } else { None };
            store.add_fact(
                &summary,
                target_level,
                group[0].domain.as_deref(),
                module,
                None,
                None,
                None,
                None,
                0.85,
                "consolidation",
                None,
            )?;
            summaries.push(summary);
            merged += group.len() as i64;
            promoted += 1;
            for f in &group {
                store.archive_fact(&f.id)?;
            }
        }
    }
    Ok((merged, promoted, summaries))
}

fn find_duplicates(facts: &[Fact], threshold: f64) -> Vec<String> {
    let mut duplicates = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new(); // (content_key, fact_id)
    for fact in facts {
        let key = char_slice(&fact.content.to_lowercase(), 0, 100);
        let mut found = false;
        for (existing_key, _) in &seen {
            if jaccard(&key, existing_key) >= threshold {
                duplicates.push(fact.id.clone());
                found = true;
                break;
            }
        }
        if !found {
            seen.push((key, fact.id.clone()));
        }
    }
    duplicates
}