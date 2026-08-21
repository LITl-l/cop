//! cop-ctags — the universal fallback Code Oracle Protocol plugin.
//!
//! Backed by Universal Ctags, which covers 135 languages in the version measured
//! for this specification. Answers at `structural` confidence: ctags has no scope
//! resolution and no type inference, so same-named symbols in different scopes can
//! be conflated (SPEC 6.4).
//!
//! This is the plugin the host falls back to when no language-specific plugin
//! claims a file. It is also the worked example for SPEC Appendix A.

use cop_core::*;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::BufRead;
use std::process::Command;

const SCHEME: &str = "scip-ctags";

fn engine() -> Engine {
    Engine { name: "cop-ctags".into(), version: env!("CARGO_PKG_VERSION").into() }
}

/// ctags kind name -> COP SymbolKind (SPEC 6.6).
fn sym_kind(k: &str) -> &'static str {
    match k {
        "function" | "func" | "subroutine" => "function",
        "method" => "method",
        "member" | "field" => "field",
        "class" => "class",
        "struct" | "union" => "struct",
        "interface" => "interface",
        "enum" => "enum",
        "enumerator" | "enumConstant" => "enum_member",
        "typedef" | "alias" | "type" => "type_alias",
        "variable" => "variable",
        "constant" => "constant",
        "macro" => "macro",
        "namespace" => "namespace",
        "module" => "module",
        "package" => "package",
        "property" => "property",
        "trait" => "trait",
        _ => "unknown",
    }
}

#[derive(Debug, Clone)]
struct Tag {
    name: String,
    path: String,
    line: u32,
    kind: &'static str,
    symbol: String,
}

impl Tag {
    fn location(&self) -> Location {
        Location {
            path: self.path.clone(),
            range: Range::on_line(self.line, 0, &self.name),
            is_definition: true,
        }
    }
}

struct Index {
    by_name: BTreeMap<String, Vec<Tag>>,
    by_symbol: BTreeMap<String, Tag>,
    file_count: usize,
    /// The ctags build that produced this index. Reported in `used_inputs` so a
    /// host cache is invalidated when the tool changes, not only when files do.
    tool_version: String,
}

impl Index {
    fn build(root: &str) -> std::io::Result<Self> {
        let out = Command::new("ctags")
            .args(["-R", "--output-format=json", "--fields=+nKS", "-f", "-", root])
            .output()?;
        let mut by_name: BTreeMap<String, Vec<Tag>> = BTreeMap::new();
        let mut by_symbol = BTreeMap::new();
        let mut files = std::collections::BTreeSet::new();

        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            if v.get("_type").and_then(|t| t.as_str()) != Some("tag") {
                continue;
            }
            let (Some(name), Some(path)) =
                (v.get("name").and_then(|x| x.as_str()), v.get("path").and_then(|x| x.as_str()))
            else {
                continue;
            };
            let rel = pathdiff(path, root);
            files.insert(rel.clone());
            let kind = sym_kind(v.get("kind").and_then(|x| x.as_str()).unwrap_or(""));
            let scope = v.get("scope").and_then(|x| x.as_str());
            // ctags lines are 1-based; COP positions are 0-based (SPEC 6.3).
            let line = v.get("line").and_then(|x| x.as_u64()).unwrap_or(1).saturating_sub(1) as u32;
            let tag = Tag {
                name: name.to_string(),
                path: rel.clone(),
                line,
                kind,
                symbol: make_symbol(SCHEME, &rel, scope, name, kind),
            };
            by_symbol.insert(tag.symbol.clone(), tag.clone());
            by_name.entry(name.to_string()).or_default().push(tag);
        }

        // SPEC 10.1: unspecified order does not license nondeterministic order.
        for v in by_name.values_mut() {
            v.sort_by(|a, b| (&a.path, a.line, &a.name).cmp(&(&b.path, b.line, &b.name)));
        }
        Ok(Index { by_name, by_symbol, file_count: files.len(), tool_version: ctags_version() })
    }

    fn lookup(&self, r: &SymbolRef) -> Vec<&Tag> {
        let (scope, name) = r.split();
        let mut hits: Vec<&Tag> = self.by_name.get(name).map(|v| v.iter().collect()).unwrap_or_default();
        if let Some(sc) = scope {
            let scoped: Vec<&Tag> = hits
                .iter()
                .copied()
                .filter(|t| t.symbol.contains(&format!("{sc}#")))
                .collect();
            if !scoped.is_empty() {
                hits = scoped;
            }
        }
        if let Some(k) = r.kind_hint.as_deref().filter(|k| *k != "unknown") {
            let typed: Vec<&Tag> = hits.iter().copied().filter(|t| t.kind == k).collect();
            if !typed.is_empty() {
                hits = typed;
            }
        }
        hits
    }

    /// SPEC 5.6. Everything an answer read: the tool, plus the files behind its
    /// evidence. No digest — the host owns the cache and decides how to key it.
    fn inputs_for(&self, evidence: &[Location]) -> Vec<UsedInput> {
        let mut v = vec![UsedInput::tool("ctags", &self.tool_version)];
        let mut seen = std::collections::BTreeSet::new();
        for loc in evidence {
            if seen.insert(loc.path.clone()) {
                v.push(UsedInput::file(&loc.path));
            }
        }
        v
    }

    /// SPEC 5.6, for answers that assert something about the *absence* of other
    /// symbols. `resolve` returning one symbol is a claim that nothing else matched,
    /// so a file the answer never mentions can still invalidate it; the index stands
    /// in for that negative space.
    fn inputs_for_scope(&self, evidence: &[Location]) -> Vec<UsedInput> {
        let mut v = self.inputs_for(evidence);
        v.push(UsedInput::index("ctags-tags", "workspace"));
        v
    }

    /// SPEC 5.6. A negative answer depended on the whole index, not on any file.
    fn inputs_for_miss(&self) -> Vec<UsedInput> {
        vec![
            UsedInput::tool("ctags", &self.tool_version),
            UsedInput::index("ctags-tags", "workspace"),
        ]
    }

    fn searched(&self) -> Searched {
        Searched {
            scope: Some("workspace".into()),
            roots: vec![".".into()],
            file_count: Some(self.file_count),
            index_commit: None,
            note: Some("ctags tag index; no scope or type resolution".into()),
        }
    }
}

/// `ctags --version` prints its build on the first line; anything else means we
/// are talking to a ctags we cannot identify, and saying so beats guessing.
fn ctags_version() -> String {
    Command::new("ctags")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(str::trim).map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn pathdiff(path: &str, root: &str) -> String {
    let p = std::path::Path::new(path);
    let r = std::path::Path::new(root);
    p.strip_prefix(r).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

#[derive(Serialize)]
struct VerbSpec {
    confidence: Confidence,
    requires_build: bool,
}

fn capabilities(id: &str) -> Answer {
    let v = |c: Confidence| VerbSpec { confidence: c, requires_build: false };
    Answer::ok(
        id,
        &engine(),
        Confidence::Structural,
        json!({
            "cop_versions": [COP_VERSION],
            "engine": engine(),
            // SPEC Appendix A: "*" marks a fallback plugin, used only where no
            // language-specific plugin claims the file.
            "languages": ["*"],
            "verbs": {
                "exists":   v(Confidence::Structural),
                "resolve":  v(Confidence::Structural),
                "location": v(Confidence::Structural),
            },
            "cache_keys": [],
            "limits": { "suggested_timeout_ms": 60000, "max_batch": 1024 }
        }),
        vec![], // SPEC 5.4: capabilities is evidence-exempt
    )
}

fn handle(req: &Request, index: &Index) -> Answer {
    let id = &req.id;
    let e = engine();
    let p = &req.params;
    let get_ref = || serde_json::from_value::<SymbolRef>(p.get("ref").cloned().unwrap_or(json!(null))).ok();

    match req.verb.as_str() {
        "capabilities" => capabilities(id),

        "exists" | "resolve" => {
            // `exists` may be asked by SymbolId instead of by ref (SPEC 7.3).
            if let (Some(sym), None) = (p.get("symbol").and_then(|s| s.as_str()), p.get("ref")) {
                return match index.by_symbol.get(sym) {
                    None => Answer::not_found(id, &e, Confidence::Structural, index.searched())
                        .with_inputs(index.inputs_for_miss()),
                    Some(t) => Answer::ok(
                        id,
                        &e,
                        Confidence::Structural,
                        json!({ "exists": true, "kind": t.kind }),
                        vec![t.location()],
                    )
                    .with_inputs(index.inputs_for(&[t.location()])),
                };
            }
            let Some(r) = get_ref() else {
                return Answer::error(id, &e, "missing 'ref' or 'symbol' in params");
            };
            let hits = index.lookup(&r);
            if hits.is_empty() {
                return Answer::not_found(id, &e, Confidence::Structural, index.searched())
                    .with_inputs(index.inputs_for_miss());
            }
            let ev: Vec<Location> = hits.iter().take(8).map(|t| t.location()).collect();
            if req.verb == "exists" {
                // Existence survives new files, so only the files behind the hits matter.
                return Answer::ok(
                    id,
                    &e,
                    Confidence::Structural,
                    json!({ "exists": true, "kind": hits[0].kind }),
                    ev.clone(),
                )
                .with_inputs(index.inputs_for(&ev));
            }
            if hits.len() > 1 {
                return Answer::ambiguous(
                    id,
                    &e,
                    Confidence::Structural,
                    hits.iter()
                        .take(8)
                        .map(|t| Candidate {
                            symbol: t.symbol.clone(),
                            kind: Some(t.kind.into()),
                            evidence: vec![t.location()],
                        })
                        .collect(),
                )
                .with_inputs(index.inputs_for_scope(&ev));
            }
            Answer::ok(
                id,
                &e,
                Confidence::Structural,
                json!({ "symbol": hits[0].symbol, "kind": hits[0].kind }),
                vec![hits[0].location()],
            )
            .with_inputs(index.inputs_for_scope(&ev))
        }

        "location" => {
            let sym = p.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            match index.by_symbol.get(sym) {
                None => Answer::not_found(id, &e, Confidence::Structural, index.searched())
                    .with_inputs(index.inputs_for_miss()),
                Some(t) => Answer::ok(
                    id,
                    &e,
                    Confidence::Structural,
                    json!({ "locations": [t.location()] }),
                    vec![t.location()],
                )
                .with_inputs(index.inputs_for(&[t.location()])),
            }
        }

        // SPEC 9.2: unknown or unimplemented verbs are `unsupported`, never a crash
        // and never `not_found`.
        other => Answer::unsupported(id, &e, format!("verb '{other}' not implemented by cop-ctags")),
    }
}

fn main() -> std::process::ExitCode {
    let root = std::env::var("COP_WORKSPACE").unwrap_or_else(|_| ".".into());
    let stdin = std::io::stdin();
    // covers: COP-FD-USE COP-WIRE-CHANNEL
    let mut out = ProtocolChannel::open();
    // Lazily built: `capabilities` must not pay for indexing (SPEC 4.4).
    let mut index: Option<Index> = None;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // covers: COP-BATCH-ACCEPT COP-BATCH-EMPTY
        let reqs = match parse_line(line) {
            Ok(r) => r,
            Err(err) => {
                // SPEC 9.3: an uncorrelatable line cannot be answered.
                eprintln!("cop-ctags: unparseable input: {err}");
                return std::process::ExitCode::from(2);
            }
        };
        for req in &reqs {
            if req.verb != "capabilities" && index.is_none() {
                index = Some(match Index::build(&root) {
                    Ok(i) => i,
                    Err(err) => {
                        eprintln!("cop-ctags: ctags failed: {err}");
                        return std::process::ExitCode::from(1);
                    }
                });
            }
            let ans = match &index {
                Some(i) => handle(req, i),
                None => capabilities(&req.id),
            };
            let _ = out.send(&ans);
        }
    }
    // SPEC 4.5: exit code describes the plugin, never the answers.
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an index by hand. Going through `ctags` would test ctags; these
    /// tests are about what this plugin does with tags once it has them.
    fn index_of(tags: &[(&str, &str, &str, &str)]) -> Index {
        let mut by_name: BTreeMap<String, Vec<Tag>> = BTreeMap::new();
        let mut by_symbol = BTreeMap::new();
        for (i, (name, path, kind, scope)) in tags.iter().enumerate() {
            let sc = if scope.is_empty() { None } else { Some(*scope) };
            let t = Tag {
                name: (*name).into(),
                path: (*path).into(),
                line: i as u32,
                kind: sym_kind(kind),
                symbol: make_symbol(SCHEME, path, sc, name, sym_kind(kind)),
            };
            by_symbol.insert(t.symbol.clone(), t.clone());
            by_name.entry((*name).into()).or_default().push(t);
        }
        Index { by_name, by_symbol, file_count: 1, tool_version: "test".into() }
    }

    fn sref(name: &str, kind_hint: Option<&str>) -> SymbolRef {
        let mut v = json!({ "name": name });
        if let Some(k) = kind_hint {
            v["kind_hint"] = json!(k);
        }
        serde_json::from_value(v).unwrap()
    }

    /// A scope in the reference narrows the hits — but only when it matches
    /// something. ctags has no scope resolution (SPEC 6.4), so treating a
    /// non-matching scope as "no results" would turn the plugin's ignorance
    /// into a claim of absence, which SPEC 3.4 forbids.
    #[test]
    fn scope_narrows_only_when_it_matches() {
        let ix = index_of(&[
            ("greet", "a.py", "method", "Greeter"),
            ("greet", "b.py", "function", ""),
        ]);
        let scoped = ix.lookup(&sref("Greeter.greet", None));
        assert_eq!(scoped.len(), 1, "a matching scope must narrow");
        assert_eq!(scoped[0].path, "a.py");

        let unmatched = ix.lookup(&sref("Nowhere.greet", None));
        assert_eq!(unmatched.len(), 2, "an unmatched scope must not delete the hits");
    }

    /// Same rule for `kind_hint`: a filter that matches nothing is a filter the
    /// plugin cannot honour, not proof the symbol is missing.
    #[test]
    fn kind_hint_narrows_only_when_it_matches() {
        let ix = index_of(&[("Config", "a.py", "class", ""), ("Config", "b.py", "variable", "")]);
        assert_eq!(ix.lookup(&sref("Config", Some("class"))).len(), 1);
        assert_eq!(ix.lookup(&sref("Config", Some("interface"))).len(), 2);
        assert_eq!(ix.lookup(&sref("Config", Some("unknown"))).len(), 2);
    }

    /// SPEC 5.6. The host caches on what an answer read, so an answer that
    /// under-reports its inputs is worse than one that reports none: the first
    /// produces a cache entry nothing can invalidate.
    #[test]
    fn used_inputs_name_the_tool_and_every_distinct_file() {
        let ix = index_of(&[("greet", "a.py", "function", "")]);
        let ev = vec![
            Location { path: "a.py".into(), range: Range::on_line(0, 0, "greet"), is_definition: true },
            Location { path: "a.py".into(), range: Range::on_line(9, 0, "greet"), is_definition: true },
            Location { path: "b.py".into(), range: Range::on_line(0, 0, "greet"), is_definition: true },
        ];
        let got = ix.inputs_for(&ev);
        let tool = got.iter().find(|i| i.kind == "tool").expect("the tool is an input");
        assert_eq!(tool.name.as_deref(), Some("ctags"));
        assert_eq!(tool.version.as_deref(), Some("test"));
        let files: Vec<&str> = got.iter().filter(|i| i.kind == "file").filter_map(|i| i.path.as_deref()).collect();
        assert_eq!(files, ["a.py", "b.py"], "each file once, in the order first read");
        assert!(got.iter().all(|i| i.kind != "index"), "a positive answer about one file is not about the index");
    }

    /// SPEC 5.6 COP-CACHE-MISS. A miss depends on everything searched, so it
    /// names the index and no file: a miss keyed on files can never be
    /// invalidated by the file whose creation would fix it.
    #[test]
    fn a_miss_names_the_index_and_no_file() {
        let ix = index_of(&[("greet", "a.py", "function", "")]);
        let got = ix.inputs_for_miss();
        assert!(got.iter().any(|i| i.kind == "tool"));
        let idx = got.iter().find(|i| i.kind == "index").expect("a miss names its index");
        assert_eq!(idx.scope.as_deref(), Some("workspace"));
        assert!(got.iter().all(|i| i.kind != "file"));
    }

    /// A unique `resolve` asserts that nothing else matched, which is a claim
    /// about the whole index and not only about the file it points at.
    #[test]
    fn a_uniqueness_claim_also_names_the_index() {
        let ix = index_of(&[("greet", "a.py", "function", "")]);
        let ev = vec![Location {
            path: "a.py".into(),
            range: Range::on_line(0, 0, "greet"),
            is_definition: true,
        }];
        let got = ix.inputs_for_scope(&ev);
        assert!(got.iter().any(|i| i.kind == "index"));
        assert!(got.iter().any(|i| i.path.as_deref() == Some("a.py")));
    }

    /// The reported tool version keys the host's cache. An empty or invented
    /// string would key it wrongly and silently, which is the failure mode a
    /// cache is least able to recover from.
    #[test]
    fn the_reported_ctags_version_identifies_ctags() {
        let v = ctags_version();
        assert!(!v.is_empty(), "an unidentifiable tool must still say so");
        // Accepting "unknown" unconditionally would let a version probe that
        // never works pass forever, so where ctags is present the probe must
        // actually name it. Mutation testing found exactly that hole.
        let present = Command::new("ctags").arg("--version").output().is_ok_and(|o| !o.stdout.is_empty());
        if present {
            assert!(v.to_lowercase().contains("ctags"), "ctags is installed but the probe said {v:?}");
        } else {
            assert_eq!(v, "unknown", "no ctags: the probe must say so, not invent a version");
        }
    }

    /// The ctags-kind mapping is a pure lookup table, and a table is only tested
    /// by naming every entry. Mutation testing found that the fixture workspace
    /// exercises five kinds out of eighteen, so deleting most arms changed
    /// nothing observable — a real gap, not a scoring artefact.
    #[test]
    fn every_ctags_kind_maps_to_its_symbol_kind() {
        // covers: COP-CONF-HANDSHAKE
        let expected: &[(&str, &str)] = &[
            ("function", "function"),
            ("func", "function"),
            ("subroutine", "function"),
            ("method", "method"),
            ("member", "field"),
            ("field", "field"),
            ("class", "class"),
            ("struct", "struct"),
            ("union", "struct"),
            ("interface", "interface"),
            ("enum", "enum"),
            ("enumerator", "enum_member"),
            ("enumConstant", "enum_member"),
            ("typedef", "type_alias"),
            ("alias", "type_alias"),
            ("type", "type_alias"),
            ("variable", "variable"),
            ("constant", "constant"),
            ("macro", "macro"),
            ("namespace", "namespace"),
            ("module", "module"),
            ("package", "package"),
            ("property", "property"),
            ("trait", "trait"),
        ];
        for (ctags, cop) in expected {
            assert_eq!(sym_kind(ctags), *cop, "ctags kind {ctags:?}");
        }
        // Anything unrecognised is `unknown`, never a guess (SPEC 6.6).
        for unknown in ["", "xyzzy", "anonMember", "local"] {
            assert_eq!(sym_kind(unknown), "unknown", "{unknown:?} must not be guessed");
        }
    }

    #[test]
    fn symbols_use_the_scip_grammar() {
        // covers: COP-CONF-HANDSHAKE
        let s = make_symbol(SCHEME, "src/a/b.py", Some("Cls"), "meth", "method");
        assert_eq!(s, "scip-ctags . . . src/a/b.py/Cls#meth().");
        assert_eq!(s.split(' ').count(), 5);
        let free = make_symbol(SCHEME, "src/a.py", None, "f", "function");
        assert!(free.ends_with("f()."), "callables carry the method descriptor");
    }
}
