//! Unit tests (#7): pure algorithms + in-memory store/search behavior.

use crate::store::{Store, TtlConfig};
use crate::util::{char_slice, cosine, jaccard, parse_iso, py_round, thousands, title_case};
use crate::{consolidate, search, seqmatch};

fn emb(v: &[f32]) -> Vec<f32> {
    let mut e = vec![0.0f32; 8];
    for (i, x) in v.iter().enumerate() {
        e[i] = *x;
    }
    e
}

#[test]
fn seqmatch_known_values() {
    assert!((seqmatch::ratio("abcd", "bcde") - 0.75).abs() < 1e-12);
    assert!((seqmatch::ratio("", "") - 1.0).abs() < 1e-12);
    assert!((seqmatch::ratio("abc", "") - 0.0).abs() < 1e-12);
    assert!((seqmatch::ratio("hello world", "hello world") - 1.0).abs() < 1e-12);
    // near-duplicate (one char differs) should be >= 0.90 (dedup gate threshold)
    assert!(seqmatch::ratio("iteration 0 test data", "iteration 1 test data") >= 0.90);
}

#[test]
fn util_cosine() {
    assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-9);
    assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-9);
    assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-9);
    assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
}

#[test]
fn util_jaccard() {
    assert!((jaccard("a b c", "a b d") - 0.5).abs() < 1e-9);
    assert_eq!(jaccard("", ""), 0.0);
    assert!((jaccard("a b", "a b") - 1.0).abs() < 1e-9);
}

#[test]
fn util_misc() {
    assert_eq!(char_slice("привет мир", 0, 6), "привет");
    assert_eq!(title_case("nodejs"), "Nodejs");
    assert_eq!(title_case("unknown"), "Unknown");
    assert_eq!(thousands(1234567), "1,234,567");
    assert_eq!(thousands(500), "500");
    // banker's rounding
    assert_eq!(py_round(2.5, 0), 2.0);
    assert_eq!(py_round(3.5, 0), 4.0);
    assert!(parse_iso("2026-11-12T10:00:00").is_some());
    assert!(parse_iso("2026-11-12").is_some());
    assert!(parse_iso("not-a-date").is_none());
}

#[test]
fn store_add_cache_and_stats() {
    let s = Store::new(":memory:").unwrap();
    let id1 = s.add_fact("alpha fact about postgres", 1, Some("infra"), None, None, None, None, Some(emb(&[1.0, 0.0])), 1.0, "manual", None).unwrap();
    assert_eq!(s.get_facts_with_embeddings().unwrap().len(), 1);
    // cache must reflect a new write
    let _id2 = s.add_fact("beta fact about vpn", 1, Some("infra"), None, None, None, None, Some(emb(&[0.0, 1.0])), 1.0, "manual", None).unwrap();
    assert_eq!(s.get_facts_with_embeddings().unwrap().len(), 2);
    let stats = s.get_stats().unwrap();
    assert_eq!(stats["total_facts"], 2);
    // soft delete removes from the embedded pool
    assert!(s.delete_fact(&id1).unwrap());
    assert_eq!(s.get_facts_with_embeddings().unwrap().len(), 1);
    assert_eq!(s.get_stats().unwrap()["archived_facts"], 1);
}

#[test]
fn store_stale_and_ttl() {
    let s = Store::new(":memory:").unwrap();
    let id = s.add_fact("ttl zero fact", 1, Some("t"), None, None, None, Some(TtlConfig { ttl_seconds: 0, refresh_trigger: None, on_expire: "mark_stale".into() }), Some(emb(&[1.0, 0.0])), 1.0, "manual", None).unwrap();
    let report = crate::ttl::process_expired(&s);
    assert!(report["marked_stale"].as_i64().unwrap() >= 1);
    let f = s.get_fact(&id).unwrap().unwrap();
    assert!(f.is_stale);
    assert!(f.valid_until.is_some());
}

#[test]
fn search_keyword_gate_and_garbage() {
    let s = Store::new(":memory:").unwrap();
    s.add_fact("PENDING tasks next session fix login bug", 1, Some("workflow"), None, None, None, None, Some(emb(&[1.0, 0.0])), 1.0, "manual", None).unwrap();
    // keyword overlap -> found even without a query embedder
    let r = search::hybrid_search(&s, None, "PENDING tasks next session", 10, 0.5, 0.3, 0.2);
    assert_eq!(r["status"], "success");
    assert!(r["results"].as_array().unwrap().len() >= 1);
    // garbage -> empty (no keyword, no semantic)
    let g = search::hybrid_search(&s, None, "xyzzy12345nonsense_QQQQQ", 10, 0.5, 0.3, 0.2);
    assert_eq!(g["results"].as_array().unwrap().len(), 0);
}

#[test]
fn search_excludes_stale() {
    let s = Store::new(":memory:").unwrap();
    let id = s.add_fact("stale pending tasks login", 1, Some("workflow"), None, None, None, None, Some(emb(&[1.0, 0.0])), 1.0, "manual", None).unwrap();
    s.mark_stale(&id).unwrap();
    let r = search::hybrid_search(&s, None, "pending tasks login", 10, 0.5, 0.3, 0.2);
    assert_eq!(r["results"].as_array().unwrap().len(), 0);
}

#[test]
fn consolidate_merges_l3() {
    let s = Store::new(":memory:").unwrap();
    for i in 0..6 {
        s.add_fact(&format!("code detail number {i} about module"), 3, Some("d"), Some("m"), None, None, None, None, 1.0, "manual", None).unwrap();
    }
    let r = consolidate::consolidate(&s, 5).unwrap();
    assert!(r["merged_count"].as_i64().unwrap() >= 6);
    assert!(r["promoted_count"].as_i64().unwrap() >= 1);
}

#[test]
fn coldstart_detects_rust() {
    let dir = std::env::temp_dir().join(format!("rlmrs-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let ptype = crate::coldstart::detect_project_type_public(&dir);
    assert_eq!(ptype, "rust");
    std::fs::remove_dir_all(&dir).ok();
}