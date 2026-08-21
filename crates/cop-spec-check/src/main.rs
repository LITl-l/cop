//! cop-spec-check — validate every JSON example in SPEC.md against the wire schema.
//!
//! A specification whose own examples do not validate is a specification with a
//! bug in it. Envelope examples (those carrying `cop` and `id`) are checked
//! against the Request/Answer schema; `**params:**` blocks are wrapped in a
//! request envelope so that the per-verb dispatch is exercised; `**result:**`
//! blocks are checked against the `$defs` entry for their section.

use serde_json::{json, Value};
use std::collections::BTreeMap;

fn verb_for_section(s: &str) -> Option<&'static str> {
    Some(match s {
        "7.1" => "capabilities",
        "7.2" => "resolve",
        "7.3" => "exists",
        "7.4" => "location",
        "7.5" => "signature",
        "7.6" => "value",
        "7.7" => "members",
        "7.8" => "docstring",
        "7.9" => "visibility",
        "7.10" => "parse_snippet",
        "7.11" => "references",
        _ => return None,
    })
}

fn result_def_for_section(s: &str) -> Option<&'static str> {
    Some(match s {
        "7.1" => "CapabilitiesResult",
        "7.2" => "ResolveResult",
        "7.3" => "ExistsResult",
        "7.4" => "LocationResult",
        "7.5" => "SignatureResult",
        "7.6" => "ValueResult",
        "7.7" => "MembersResult",
        "7.8" => "DocstringResult",
        "7.9" => "VisibilityResult",
        "7.10" => "ParseSnippetResult",
        "7.11" => "ReferencesResult",
        _ => return None,
    })
}

/// Leading `## 7.10 ...` or `### 5.2 ...` -> `Some("7.10")`.
fn section_of(line: &str) -> Option<String> {
    let t = line.trim_start();
    if !t.starts_with('#') {
        return None;
    }
    let rest = t.trim_start_matches('#').trim_start();
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let num = num.trim_end_matches('.').to_string();
    if num.contains('.') && num.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Some(num)
    } else {
        None
    }
}

struct Block {
    section: Option<String>,
    role: Option<String>,
    line: usize,
    body: String,
}

/// A normative rule: `COP-AREA-NAME` (CLASS) — text with MUST/SHOULD/MAY.
struct Rule {
    id: String,
    class: String,
    level: String,
    /// Source line, for future `--emit-pics` output.
    #[allow(dead_code)]
    line: usize,
}

fn extract_rules(spec: &str) -> Vec<Rule> {
    let re_line = |l: &str| -> Option<(String, String)> {
        // matches: `COP-X-Y` (PLUGIN)  /  | `COP-X-Y` | ...
        let start = l.find("COP-")?;
        let rest = &l[start..];
        let end = rest
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-'))
            .unwrap_or(rest.len());
        let id = rest[..end].trim_end_matches('-').to_string();
        if id.matches('-').count() < 2 {
            return None;
        }
        let class = ["HOST, PLUGIN", "PLUGIN", "HOST"]
            .iter()
            .find(|c| l.contains(&format!("({c})")))
            .map(|c| c.to_string())
            .unwrap_or_else(|| "UNSPECIFIED".into());
        Some((id, class))
    };

    let mut out: Vec<Rule> = Vec::new();
    let mut in_conf_table = false;
    for (i, l) in spec.lines().enumerate() {
        if l.contains("A **conformant plugin**") {
            in_conf_table = true;
        }
        if in_conf_table && l.starts_with("All of the above are") {
            in_conf_table = false;
        }
        let Some((id, mut class)) = re_line(l) else { continue };
        if out.iter().any(|r| r.id == id) {
            continue;
        }
        if class == "UNSPECIFIED" && in_conf_table {
            class = "PLUGIN".into(); // the table declares the class once, below it
        }
        // The RFC 2119 keyword can land on a later line of the same paragraph,
        // so read the whole paragraph. Reading only the rule's own line makes
        // the level depend on where the text happened to wrap, which is a
        // property of the editor and not of the specification.
        // A blank line ends the paragraph, and so does the next bullet: a list of
        // rules has no blank lines between items, and swallowing the next item
        // would give this rule its neighbour's level.
        let para: String = spec
            .lines()
            .skip(i)
            .enumerate()
            .take_while(|(j, x)| {
                !x.trim().is_empty() && (*j == 0 || !x.trim_start().starts_with("- "))
            })
            .map(|(_, x)| x)
            .collect::<Vec<_>>()
            .join(" ");
        let level = if para.contains("MUST NOT") || para.contains("MUST") {
            "MUST"
        } else if para.contains("SHOULD") {
            "SHOULD"
        } else if para.contains("MAY") {
            "MAY"
        } else if in_conf_table {
            "MUST"
        } else {
            "UNSPECIFIED"
        };
        out.push(Rule { id, class, level: level.into(), line: i + 1 });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Scan the tree for `covers: COP-...` annotations.
fn extract_covers(root: &std::path::Path) -> BTreeMap<String, Vec<String>> {
    fn walk(p: &std::path::Path, acc: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut es: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        es.sort();
        for e in es {
            let name = e.file_name().unwrap_or_default().to_string_lossy().to_string();
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            if e.is_dir() {
                walk(&e, acc);
            } else if matches!(
                e.extension().and_then(|x| x.to_str()),
                Some("rs" | "ts" | "js" | "json" | "toml" | "sh")
            ) {
                acc.push(e);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for (i, line) in text.lines().enumerate() {
            let Some(pos) = line.find("covers:") else { continue };
            for tok in line[pos + 7..].split(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')) {
                if tok.starts_with("COP-") && tok.matches('-').count() >= 2 {
                    map.entry(tok.to_string())
                        .or_default()
                        .push(format!("{}:{}", f.display(), i + 1));
                }
            }
        }
    }
    map
}

fn coverage_mode(spec_path: &str) -> std::process::ExitCode {
    let spec = std::fs::read_to_string(spec_path).expect("SPEC.md readable");
    let rules = extract_rules(&spec);
    let covers = extract_covers(std::path::Path::new("."));

    // Rules are gated by class. The conformance suite can test everything asked of
    // a PLUGIN, and it can test the host behaviour the hostile suite drives. Rules
    // that constrain a host feature this repository does not implement (the
    // capability enforcement of SPEC 12) are reported as PENDING-HOST rather than
    // quietly counted as covered — the number is the point.
    const PENDING_HOST: &[&str] = &["COP-CAP-GRANT", "COP-CAP-PIN", "COP-CAP-DISCLOSE"];

    let (mut uncovered_must, mut pending, mut unspecified_class) = (0usize, 0usize, 0usize);
    println!("{:<24} {:<9} {:<6} {}", "RULE", "CLASS", "LEVEL", "TESTS");
    for r in &rules {
        let n = covers.get(&r.id).map(|v| v.len()).unwrap_or(0);
        let status = if n > 0 {
            format!("covered by {n}")
        } else if PENDING_HOST.contains(&r.id.as_str()) {
            pending += 1;
            "PENDING-HOST".into()
        } else if r.level == "MUST" {
            uncovered_must += 1;
            "UNCOVERED".into()
        } else {
            "-".into()
        };
        if r.class == "UNSPECIFIED" {
            unspecified_class += 1;
        }
        println!("{:<24} {:<9} {:<6} {}", r.id, r.class, r.level, status);
    }

    // A covers: tag naming a rule that does not exist is a stale test.
    let unknown: Vec<&String> = covers.keys().filter(|k| !rules.iter().any(|r| &&r.id == k)).collect();
    println!();
    let tested = rules.iter().filter(|r| covers.contains_key(&r.id)).count();
    println!("{} rules, {tested} with tests, {pending} pending the host implementation", rules.len());
    if unspecified_class > 0 {
        println!("WARN  {unspecified_class} rule(s) do not name a product class (SPEC 2.1)");
    }
    // A rule with no RFC 2119 keyword tells an implementer nothing about
    // whether they may ship without it, which is the whole job of a rule.
    let unspecified_level: Vec<&str> =
        rules.iter().filter(|r| r.level == "UNSPECIFIED").map(|r| r.id.as_str()).collect();
    for r in &unspecified_level {
        println!("FAIL  {r} states no MUST/SHOULD/MAY (SPEC 2.1)");
    }
    for u in &unknown {
        println!("FAIL  covers: {u} names no rule in the specification");
    }
    if uncovered_must > 0 {
        println!("FAIL  {uncovered_must} MUST rule(s) have no test");
    }
    if uncovered_must > 0 || !unknown.is_empty() || !unspecified_level.is_empty() {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--coverage") {
        let spec = args.iter().skip(1).find(|a| !a.starts_with("--")).cloned()
            .unwrap_or_else(|| "SPEC.md".into());
        return coverage_mode(&spec);
    }
    let spec_path = std::env::args().nth(1).unwrap_or_else(|| "SPEC.md".into());
    let schema_path = std::env::args().nth(2).unwrap_or_else(|| "schemas/cop-0.1.schema.json".into());

    let spec = std::fs::read_to_string(&spec_path).expect("SPEC.md readable");
    let schema: Value =
        serde_json::from_str(&std::fs::read_to_string(&schema_path).expect("schema readable"))
            .expect("schema is JSON");
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");

    // Per-$defs validators for bare result payloads.
    let defs = schema.get("$defs").and_then(|d| d.as_object()).cloned().unwrap_or_default();
    let mut def_validators: BTreeMap<String, jsonschema::Validator> = BTreeMap::new();
    for (name, sub) in &defs {
        // Carry only `$defs` (so `$ref` resolves) and the definition itself.
        // Merging the whole root schema would drag in the request/answer
        // dispatch, which a bare result payload is not required to satisfy.
        let mut merged = serde_json::Map::new();
        merged.insert("$schema".into(), schema["$schema"].clone());
        merged.insert("$defs".into(), Value::Object(defs.clone()));
        for (k, v) in sub.as_object().cloned().unwrap_or_default() {
            merged.insert(k, v);
        }
        if let Ok(v) = jsonschema::validator_for(&Value::Object(merged)) {
            def_validators.insert(name.clone(), v);
        }
    }

    // Collect fenced ```json blocks, tracking the section and whether the nearest
    // preceding bold marker was **params:** or **result:**.
    let lines: Vec<&str> = spec.lines().collect();
    let (mut blocks, mut section, mut role) = (Vec::new(), None::<String>, None::<String>);
    let mut i = 0;
    while i < lines.len() {
        if let Some(s) = section_of(lines[i]) {
            section = Some(s);
            role = None;
        }
        let t = lines[i].trim();
        if t.starts_with("**params") {
            role = Some("params".into());
        } else if t.starts_with("**result") {
            role = Some("result".into());
        }
        if t == "```json" {
            let mut j = i + 1;
            let mut buf = Vec::new();
            while j < lines.len() && lines[j].trim() != "```" {
                buf.push(lines[j]);
                j += 1;
            }
            blocks.push(Block {
                section: section.clone(),
                role: role.take(),
                line: i + 1,
                body: buf.join("\n"),
            });
            i = j;
        }
        i += 1;
    }

    let (mut ok, mut bad, mut skipped) = (0, 0, 0);
    for b in &blocks {
        let sec = b.section.clone().unwrap_or_else(|| "-".into());
        let doc: Value = match serde_json::from_str(&b.body) {
            Ok(v) => v,
            Err(e) => {
                println!("FAIL {spec_path}:{:>4} (§{sec}) not valid JSON: {e}", b.line);
                bad += 1;
                continue;
            }
        };
        if !doc.is_object() {
            skipped += 1;
            continue;
        }

        let (label, result) = if doc.get("cop").is_some() && doc.get("id").is_some() {
            ("envelope".to_string(), validator.validate(&doc).map_err(|e| e.to_string()))
        } else if b.role.as_deref() == Some("params") {
            match verb_for_section(&sec) {
                None => {
                    skipped += 1;
                    continue;
                }
                Some(verb) => {
                    let wrapped =
                        json!({"cop": "0.1", "id": "x", "verb": verb, "params": doc.clone()});
                    (
                        format!("params[{verb}]"),
                        validator.validate(&wrapped).map_err(|e| e.to_string()),
                    )
                }
            }
        } else if b.role.as_deref() == Some("result")
            && let Some(def) = result_def_for_section(&sec)
        {
            match def_validators.get(def) {
                None => {
                    skipped += 1;
                    continue;
                }
                Some(v) => (def.to_string(), v.validate(&doc).map_err(|e| e.to_string())),
            }
        } else {
            skipped += 1;
            continue;
        };

        match result {
            Ok(()) => {
                println!("ok   {spec_path}:{:>4} (§{sec}) {label}", b.line);
                ok += 1;
            }
            Err(msg) => {
                println!("FAIL {spec_path}:{:>4} (§{sec}) {label}: {msg}", b.line);
                bad += 1;
            }
        }
    }

    println!("\n{ok} validated, {bad} failed, {skipped} not envelope/params/result examples");
    if bad > 0 { std::process::ExitCode::from(1) } else { std::process::ExitCode::SUCCESS }
}
