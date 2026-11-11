//! Cold-start project discovery — port of ColdStartOptimizer (v2 coldstart.py)
//! with the Windows-path normalization (#20) and name/guidance fixes (#32).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};

use crate::store::Store;
use crate::util::{char_len, char_slice, thousands, title_case};

pub struct ProjectInfo {
    pub project_type: String,
    pub name: String,
    pub root_path: String,
    pub framework: Option<String>,
    pub entry_points: Vec<String>,
    pub dependencies: Vec<String>,
    pub loc_estimate: i64,
    pub file_count: i64,
    pub main_domains: Vec<String>,
}

pub struct DiscoveryResult {
    pub project_info: ProjectInfo,
    pub facts_created: i64,
    pub discovery_tokens: i64,
    pub suggested_domains: Vec<String>,
    pub warnings: Vec<String>,
}

const SIGNATURES: &[(&str, &[&str])] = &[
    ("python", &["pyproject.toml", "setup.py", "requirements.txt", "Pipfile"]),
    ("nodejs", &["package.json", "yarn.lock", "pnpm-lock.yaml"]),
    ("rust", &["Cargo.toml"]),
    ("go", &["go.mod", "go.sum"]),
    ("java", &["pom.xml", "build.gradle", "build.gradle.kts"]),
    ("csharp", &["*.csproj", "*.sln"]),
    ("cpp", &["CMakeLists.txt", "Makefile", "*.vcxproj"]),
];

const DOMAIN_KEYWORDS: &[(&str, &str)] = &[
    ("api", "api"), ("auth", "auth"), ("database", "database"), ("db", "database"),
    ("models", "models"), ("views", "frontend"), ("components", "frontend"),
    ("pages", "frontend"), ("services", "services"), ("utils", "utilities"),
    ("helpers", "utilities"), ("common", "core"), ("core", "core"), ("lib", "core"),
    ("tests", "testing"), ("test", "testing"), ("docs", "docs"), ("config", "config"),
    ("settings", "config"),
];

const DOMAIN_EXCLUDE: &[&str] = &["node_modules", "__pycache__", "venv", ".venv", "target", "build", "dist"];

const SKIP_DIRS: &[&str] = &[
    "node_modules", "__pycache__", ".venv", "venv", ".git", ".hg", ".svn", "target",
    "build", "dist", ".pytest_cache", ".mypy_cache", ".tox", "htmlcov", ".next",
    ".nuxt", "vendor", "bower_components",
];

fn detect_project_type(root: &Path) -> String {
    for (ptype, sigs) in SIGNATURES {
        for sig in *sigs {
            if let Some(ext) = sig.strip_prefix('*') {
                if let Ok(entries) = std::fs::read_dir(root) {
                    for e in entries.flatten() {
                        if e.file_name().to_string_lossy().ends_with(ext) {
                            return ptype.to_string();
                        }
                    }
                }
            } else if root.join(sig).exists() {
                return ptype.to_string();
            }
        }
    }
    "unknown".to_string()
}

fn gather_project_info(root: &Path, ptype: &str) -> ProjectInfo {
    let mut name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut framework: Option<String> = None;
    let mut dependencies: Vec<String> = Vec::new();
    let mut entry_points: Vec<String> = Vec::new();

    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default();

    if ptype == "python" {
        let pyproject = root.join("pyproject.toml");
        if pyproject.exists() {
            let content = read("pyproject.toml");
            let name_re = regex::Regex::new(r#"name\s*=\s*["']([^"']+)["']"#).unwrap();
            if let Some(c) = name_re.captures(&content) {
                name = c[1].to_string();
            }
            for (fw, pat) in [
                ("fastapi", r#""fastapi"|fastapi"#),
                ("django", r#""django"|django"#),
                ("flask", r#""flask"|flask"#),
                ("streamlit", r#""streamlit"|streamlit"#),
                ("pytorch", r#""torch"|torch"#),
                ("tensorflow", r#""tensorflow"|tensorflow"#),
            ] {
                if regex::Regex::new(&format!("(?i){pat}")).unwrap().is_match(&content) {
                    framework = Some(fw.to_string());
                    break;
                }
            }
            let dep_re = regex::Regex::new(r#"(?m)^\s*["']([a-zA-Z][a-zA-Z0-9_-]+)"#).unwrap();
            let mut seen = std::collections::HashSet::new();
            for c in dep_re.captures_iter(&content) {
                let d = c[1].to_string();
                if seen.insert(d.clone()) {
                    dependencies.push(d);
                }
                if dependencies.len() >= 20 {
                    break;
                }
            }
        }
        for ep in ["main.py", "app.py", "__main__.py", "cli.py"] {
            if root.join(ep).exists() {
                entry_points.push(ep.to_string());
            }
        }
    } else if ptype == "nodejs" {
        let pj = root.join("package.json");
        if pj.exists() {
            if let Ok(data) = serde_json::from_str::<Value>(&read("package.json")) {
                if let Some(n) = data.get("name").and_then(|v| v.as_str()) {
                    name = n.to_string();
                }
                let mut all_deps: Vec<String> = Vec::new();
                for key in ["dependencies", "devDependencies"] {
                    if let Some(obj) = data.get(key).and_then(|v| v.as_object()) {
                        for k in obj.keys() {
                            all_deps.push(k.clone());
                        }
                    }
                }
                // Python: outer loop has no break — last matching framework wins
                for (fw, pat) in [
                    ("react", r#""react""#),
                    ("vue", r#""vue""#),
                    ("next", r#""next""#),
                    ("express", r#""express""#),
                    ("nestjs", r#""@nestjs/core""#),
                    ("angular", r#""@angular/core""#),
                ] {
                    let re = regex::Regex::new(pat).unwrap();
                    for dep in &all_deps {
                        if re.is_match(dep) {
                            framework = Some(fw.to_string());
                            break;
                        }
                    }
                }
                dependencies = all_deps.into_iter().take(20).collect();
                if let Some(m) = data.get("main").and_then(|v| v.as_str()) {
                    entry_points.push(m.to_string());
                }
            }
        }
    } else if ptype == "rust" {
        let ct = root.join("Cargo.toml");
        if ct.exists() {
            let content = read("Cargo.toml");
            let name_re = regex::Regex::new(r#"name\s*=\s*"([^"]+)""#).unwrap();
            if let Some(c) = name_re.captures(&content) {
                name = c[1].to_string();
            }
            for (fw, pat) in [("actix", "actix-web"), ("axum", "axum"), ("rocket", "rocket"), ("tokio", "tokio")] {
                if regex::Regex::new(&format!("(?i){pat}")).unwrap().is_match(&content) {
                    framework = Some(fw.to_string());
                    break;
                }
            }
            if root.join("src/main.rs").exists() {
                entry_points.push("src/main.rs".to_string());
            }
            if root.join("src/lib.rs").exists() {
                entry_points.push("src/lib.rs".to_string());
            }
        }
    }

    let (file_count, loc_estimate) = count_files_and_loc(root, ptype);

    ProjectInfo {
        project_type: ptype.to_string(),
        name,
        root_path: root.to_string_lossy().into_owned(),
        framework,
        entry_points,
        dependencies,
        loc_estimate,
        file_count,
        main_domains: Vec::new(),
    }
}

fn extensions_for(ptype: &str) -> &'static [&'static str] {
    match ptype {
        "python" => &[".py"],
        "nodejs" => &[".js", ".ts", ".jsx", ".tsx", ".vue"],
        "rust" => &[".rs"],
        "go" => &[".go"],
        "java" => &[".java"],
        "csharp" => &[".cs"],
        "cpp" => &[".cpp", ".hpp", ".c", ".h"],
        _ => &[],
    }
}

fn count_files_and_loc(root: &Path, ptype: &str) -> (i64, i64) {
    let exts = extensions_for(ptype);
    if exts.is_empty() {
        return (0, 0);
    }
    // git ls-files first (5s timeout)
    if let Ok(mut child) = std::process::Command::new("git")
        .arg("ls-files")
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        if let Some(out) = child.stdout.take() {
                            use std::io::Read;
                            let mut s = String::new();
                            let mut out = out;
                            let _ = out.read_to_string(&mut s);
                            let count = s
                                .lines()
                                .filter(|f| !f.is_empty() && exts.iter().any(|e| f.ends_with(e)))
                                .count() as i64;
                            return (count, count * 50);
                        }
                    }
                    break;
                }
                Ok(None) => {
                    if start.elapsed() > Duration::from_secs(5) {
                        let _ = child.kill();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
    }
    // fallback walk
    let mut file_count: i64 = 0;
    fn walk(path: &Path, depth: usize, exts: &[&str], count: &mut i64) {
        if depth > 15 || *count >= 10000 {
            return;
        }
        let entries = match std::fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            if *count >= 10000 {
                break;
            }
            let p = entry.path();
            let fname = entry.file_name().to_string_lossy().to_lowercase();
            if p.is_dir() {
                if SKIP_DIRS.contains(&fname.as_str()) {
                    continue;
                }
                walk(&p, depth + 1, exts, count);
            } else if p.is_file() {
                let lname = fname;
                if exts.iter().any(|e| lname.ends_with(e)) {
                    *count += 1;
                }
            }
        }
    }
    walk(root, 0, exts, &mut file_count);
    (file_count, file_count * 50)
}

fn discover_domains(root: &Path) -> Vec<String> {
    let mut domains: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let lower = name.to_lowercase();
            if let Some((_, mapped)) = DOMAIN_KEYWORDS.iter().find(|(k, _)| *k == lower) {
                if !domains.contains(&mapped.to_string()) {
                    domains.push(mapped.to_string());
                }
            } else if !DOMAIN_EXCLUDE.contains(&lower.as_str()) && !domains.contains(&lower) {
                domains.push(lower);
            }
        }
    }
    domains.sort();
    domains.truncate(10);
    domains
}

struct TemplateFact {
    content: String,
    level: i64,
    domain: Option<String>,
    confidence: f64,
}

fn template_facts(info: &ProjectInfo) -> Vec<TemplateFact> {
    let mut facts = Vec::new();
    let type_name = title_case(&info.project_type);
    let mut overview = format!("{} is a {} project", info.name, type_name);
    if let Some(fw) = &info.framework {
        overview += &format!(" using {fw}");
    }
    if info.loc_estimate > 0 {
        overview += &format!(" (~{} LOC, {} files)", thousands(info.loc_estimate), info.file_count);
    }
    facts.push(TemplateFact { content: overview, level: 0, domain: None, confidence: 0.95 });

    if !info.entry_points.is_empty() {
        facts.push(TemplateFact {
            content: format!("Main entry points: {}", info.entry_points.join(", ")),
            level: 0, domain: None, confidence: 0.9,
        });
    }
    if !info.dependencies.is_empty() {
        let top: Vec<&str> = info.dependencies.iter().take(5).map(|s| s.as_str()).collect();
        facts.push(TemplateFact {
            content: format!("Key dependencies: {}", top.join(", ")),
            level: 1, domain: Some("dependencies".into()), confidence: 0.85,
        });
    }
    if !info.main_domains.is_empty() {
        facts.push(TemplateFact {
            content: format!("Main domains/modules: {}", info.main_domains.join(", ")),
            level: 0, domain: None, confidence: 0.85,
        });
    }

    let root = Path::new(&info.root_path);
    match info.project_type.as_str() {
        "python" => {
            if root.join("tests").exists() || root.join("test").exists() {
                facts.push(TemplateFact { content: "Has test suite in tests/ directory".into(), level: 1, domain: Some("testing".into()), confidence: 0.9 });
            }
            if root.join("docs").exists() {
                facts.push(TemplateFact { content: "Has documentation in docs/ directory".into(), level: 1, domain: Some("docs".into()), confidence: 0.9 });
            }
            if root.join(".github/workflows").exists() {
                facts.push(TemplateFact { content: "Uses GitHub Actions for CI/CD".into(), level: 1, domain: Some("devops".into()), confidence: 0.9 });
            }
        }
        "nodejs" => {
            if root.join("src").exists() {
                facts.push(TemplateFact { content: "Source code in src/ directory".into(), level: 1, domain: Some("core".into()), confidence: 0.9 });
            }
            if root.join("pages").exists() {
                facts.push(TemplateFact { content: "Uses pages-based routing (likely Next.js)".into(), level: 1, domain: Some("frontend".into()), confidence: 0.85 });
            }
        }
        "rust" => {
            if root.join("src/lib.rs").exists() {
                facts.push(TemplateFact { content: "Library crate with src/lib.rs".into(), level: 1, domain: Some("core".into()), confidence: 0.9 });
            }
            if root.join("benches").exists() {
                facts.push(TemplateFact { content: "Has benchmarks in benches/ directory".into(), level: 1, domain: Some("testing".into()), confidence: 0.9 });
            }
        }
        _ => {}
    }
    facts
}

fn focused_discovery(task_hint: &str, domains: &[String]) -> Vec<TemplateFact> {
    let task_lower = task_hint.to_lowercase();
    let kw_re = regex::Regex::new(r"\b\w+\b").unwrap();
    let keywords: Vec<String> = kw_re.find_iter(&task_lower).map(|m| m.as_str().to_string()).collect();
    let mut relevant: Vec<String> = Vec::new();
    for domain in domains {
        if keywords.iter().any(|kw| kw == domain || domain.contains(kw.as_str())) {
            relevant.push(domain.clone());
        }
    }
    if relevant.is_empty() {
        return Vec::new();
    }
    vec![TemplateFact {
        content: format!("Task '{}' likely involves: {}", char_slice(task_hint, 0, 50), relevant.join(", ")),
        level: 1, domain: None, confidence: 0.7,
    }]
}

/// discover_project with #20/#32 patches. `root` is the requested path (may be Windows).
pub fn discover_project(store: &Store, root: &str, task_hint: Option<&str>) -> Result<DiscoveryResult> {
    let container_root = std::env::var("RLM_PROJECT_ROOT").unwrap_or_else(|_| "/data".to_string());
    let original_path = root.to_string();
    let win_re = regex::Regex::new(r"^[A-Za-z]:[/\\]").unwrap();
    let effective_root = if win_re.is_match(root) || root.contains('\\') {
        tracing::info!("[#20] Windows path '{root}' -> container path '{container_root}'");
        container_root.clone()
    } else {
        root.to_string()
    };

    let root_path = PathBuf::from(&effective_root);
    let mut warnings: Vec<String> = Vec::new();
    let ptype = detect_project_type(&root_path);
    let mut info = gather_project_info(&root_path, &ptype);
    let domains = discover_domains(&root_path);
    info.main_domains = domains.clone();

    let mut templates = template_facts(&info);
    if let Some(hint) = task_hint {
        templates.extend(focused_discovery(hint, &domains));
    }

    let mut facts_created = 0i64;
    for t in &templates {
        match store.add_fact(&t.content, t.level, t.domain.as_deref(), None, None, None, None, None, t.confidence, "template", None) {
            Ok(_) => facts_created += 1,
            Err(e) => warnings.push(format!("Failed to add fact: {e}")),
        }
    }

    // #32: name fix + bind-mount guidance
    let container_basename = Path::new(&container_root)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let win_name = original_path.replace('\\', "/");
    let win_name = win_name.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    if info.name == container_basename {
        info.name = win_name.clone();
    }
    if info.file_count == 0 {
        warnings.push(format!(
            "Project files not mounted in container. Use: -v /host/path/{win_name}:{container_root}:ro"
        ));
    }

    let mut discovery_tokens: i64 = 500;
    discovery_tokens += match ptype.as_str() {
        "python" => 300,
        "nodejs" => 500,
        "rust" => 200,
        _ => 0,
    };
    for t in &templates {
        discovery_tokens += char_len(&t.content) as i64 / 4 + 10;
    }

    Ok(DiscoveryResult {
        project_info: info,
        facts_created,
        discovery_tokens,
        suggested_domains: domains,
        warnings,
    })
}

pub fn discovery_response(r: &DiscoveryResult) -> Value {
    json!({
        "status": "success",
        "project_type": r.project_info.project_type,
        "project_name": r.project_info.name,
        "framework": r.project_info.framework,
        "facts_created": r.facts_created,
        "discovery_tokens": r.discovery_tokens,
        "suggested_domains": r.suggested_domains,
        "loc_estimate": r.project_info.loc_estimate,
        "file_count": r.project_info.file_count,
        "warnings": r.warnings,
    })
}