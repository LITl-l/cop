//! Code Oracle Protocol 0.1 — envelope types.
//!
//! These types are the normative wire shapes from SPEC.md sections 5 and 6.
//! They are deliberately permissive on input (unknown fields are ignored, per
//! SPEC 5.3) and strict on output.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const COP_VERSION: &str = "0.1";

// ── SPEC 6.4 ────────────────────────────────────────────────────────────────

/// The mechanism that produced an answer, not a probability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Token or regex match. Establishes presence of a string, nothing more.
    Textual,
    /// Syntax-tree based, scope-unaware. Same-named symbols may be conflated.
    Structural,
    /// Type-aware name resolution. As reliable as the language's own tooling.
    Exact,
}

// ── SPEC 6.5 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    /// The plugin searched and the subject does not exist. A positive claim.
    NotFound,
    Ambiguous,
    /// The plugin cannot answer. Carries zero information about the code.
    Unsupported,
    Error,
}

// ── SPEC 6.3 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    /// Zero-based.
    pub line: u32,
    /// Zero-based, counted in UTF-16 code units (LSP convention).
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    /// Exclusive.
    pub end: Position,
}

impl Range {
    /// Build a single-line range from a zero-based line and a UTF-8 `&str`,
    /// converting the length to UTF-16 code units as SPEC 6.3 requires.
    pub fn on_line(line: u32, col: u32, text: &str) -> Self {
        let len: u32 = text.encode_utf16().count() as u32;
        Range {
            start: Position { line, character: col },
            end: Position { line, character: col + len },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Location {
    /// Workspace-relative, `/` separated, no leading `./`.
    pub path: String,
    pub range: Range,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_definition: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ── SPEC 6.1 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RefContext {
    pub path: Option<String>,
    pub range: Option<Range>,
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub near_paths: Vec<String>,
}

/// An imprecise reference, as written in prose.
#[derive(Debug, Clone, Deserialize)]
pub struct SymbolRef {
    pub name: String,
    pub kind_hint: Option<String>,
    pub lang: Option<String>,
    #[serde(default)]
    pub context: Option<RefContext>,
}

impl SymbolRef {
    /// `UserService.authenticate` -> (`Some("UserService")`, `"authenticate"`).
    pub fn split(&self) -> (Option<&str>, &str) {
        for sep in ["::", "->", "."] {
            if let Some(i) = self.name.rfind(sep) {
                let (head, tail) = self.name.split_at(i);
                let tail = &tail[sep.len()..];
                let scope = head.rsplit(sep).next().filter(|s| !s.is_empty());
                return (scope, tail);
            }
        }
        (None, &self.name)
    }
}

// ── SPEC 5 ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Engine {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Searched {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One thing a plugin read while producing an answer (SPEC 5.6).
///
/// `digest` is optional on purpose: the plugin reports what it touched, and the
/// host — which owns the cache — decides how to key it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsedInput {
    /// `file`, `env`, `tool`, `index`, or anything a later version adds.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

impl UsedInput {
    pub fn file(path: impl Into<String>) -> Self {
        UsedInput { kind: "file".into(), path: Some(path.into()), name: None, version: None, digest: None, scope: None }
    }
    pub fn tool(name: impl Into<String>, version: impl Into<String>) -> Self {
        UsedInput { kind: "tool".into(), path: None, name: Some(name.into()), version: Some(version.into()), digest: None, scope: None }
    }
    /// For an answer that depended on everything searched rather than on one file.
    pub fn index(name: impl Into<String>, scope: impl Into<String>) -> Self {
        UsedInput { kind: "index".into(), path: None, name: Some(name.into()), version: None, digest: None, scope: Some(scope.into()) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Location>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub cop: String,
    pub id: String,
    pub verb: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub deadline_ms: Option<u64>,
}

/// SPEC 5.2. Field presence rules are enforced by the constructors below, not
/// by the type system, because the rules are conditional on `status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub cop: String,
    pub id: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searched: Option<Searched>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
    pub engine: Engine,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// SPEC 5.6. What the plugin read to produce this answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_inputs: Vec<UsedInput>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Answer {
    fn base(id: &str, engine: &Engine, status: Status) -> Self {
        Answer {
            cop: COP_VERSION.into(),
            id: id.into(),
            status,
            confidence: None,
            result: None,
            evidence: Vec::new(),
            searched: None,
            candidates: Vec::new(),
            engine: engine.clone(),
            reason: None,
            used_inputs: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    /// Attach what was read (SPEC 5.6). Chainable so that call sites read as
    /// "this answer, and here is what it depended on".
    #[must_use]
    pub fn with_inputs(mut self, inputs: Vec<UsedInput>) -> Self {
        self.used_inputs = inputs;
        self
    }

    pub fn ok<T: Serialize>(
        id: &str,
        engine: &Engine,
        confidence: Confidence,
        result: T,
        evidence: Vec<Location>,
    ) -> Self {
        let mut a = Self::base(id, engine, Status::Ok);
        a.confidence = Some(confidence);
        a.result = Some(serde_json::to_value(result).expect("result serializes"));
        a.evidence = evidence;
        a
    }

    pub fn not_found(id: &str, engine: &Engine, confidence: Confidence, searched: Searched) -> Self {
        let mut a = Self::base(id, engine, Status::NotFound);
        a.confidence = Some(confidence);
        a.searched = Some(searched);
        a
    }

    pub fn ambiguous(
        id: &str,
        engine: &Engine,
        confidence: Confidence,
        candidates: Vec<Candidate>,
    ) -> Self {
        let mut a = Self::base(id, engine, Status::Ambiguous);
        a.confidence = Some(confidence);
        a.candidates = candidates;
        a
    }

    /// SPEC 3.4: carries no information about the code. Never a finding.
    pub fn unsupported(id: &str, engine: &Engine, reason: impl Into<String>) -> Self {
        let mut a = Self::base(id, engine, Status::Unsupported);
        a.reason = Some(reason.into());
        a
    }

    pub fn error(id: &str, engine: &Engine, reason: impl Into<String>) -> Self {
        let mut a = Self::base(id, engine, Status::Error);
        a.reason = Some(reason.into());
        a
    }
}

// ── SPEC 4.6 / 4.7 ──────────────────────────────────────────────────────────

/// The channel a plugin writes answers to: fd 3 when the host opened it,
/// otherwise stdout (SPEC 4.6, `COP-FD-USE`).
///
/// This exists because the most common practical failure of a newline-delimited
/// protocol is a library the plugin links against printing to stdout.
pub struct ProtocolChannel(Box<dyn std::io::Write + Send>);

impl ProtocolChannel {
    /// Wrap an arbitrary writer. Exists so that the framing rules can be tested
    /// without a real descriptor.
    pub fn to_writer(w: impl std::io::Write + Send + 'static) -> Self {
        ProtocolChannel(Box::new(w))
    }

    /// Which descriptor `COP_PROTOCOL_FD` selects, if any. Descriptors 0-2 are
    /// stdin/stdout/stderr and are never the protocol channel.
    pub fn selected_fd(var: Option<&str>) -> Option<i32> {
        var.and_then(|v| v.parse::<i32>().ok()).filter(|fd| *fd > 2)
    }

    pub fn open() -> Self {
        let var = std::env::var("COP_PROTOCOL_FD").ok();
        match Self::selected_fd(var.as_deref()) {
            #[cfg(unix)]
            Some(fd) => {
                use std::os::fd::FromRawFd;
                // Safety: the host promises this descriptor is open for writing.
                let f = unsafe { std::fs::File::from_raw_fd(fd) };
                ProtocolChannel(Box::new(std::io::BufWriter::new(f)))
            }
            _ => ProtocolChannel(Box::new(std::io::stdout())),
        }
    }

    /// Write one answer as exactly one line (`COP-WIRE-COMPACT`).
    pub fn send(&mut self, answer: &Answer) -> std::io::Result<()> {
        let line = serde_json::to_string(answer).expect("answer serializes");
        debug_assert!(!line.contains('\n'), "COP-WIRE-COMPACT: answers are single-line");
        self.0.write_all(line.as_bytes())?;
        self.0.write_all(b"\n")?;
        self.0.flush()
    }
}

/// One input line is either a single request or a batch array (SPEC 4.7).
/// `COP-BATCH-EMPTY`: an empty array yields no requests and is not an error.
pub fn parse_line(line: &str) -> Result<Vec<Request>, serde_json::Error> {
    let v: serde_json::Value = serde_json::from_str(line)?;
    if v.is_array() {
        serde_json::from_value(v)
    } else {
        serde_json::from_value(v).map(|r| vec![r])
    }
}

// ── SPEC 6.2 ────────────────────────────────────────────────────────────────

/// Descriptor suffix for a symbol kind: `#` for types, `().` for callables,
/// `.` for terms.
pub fn descriptor(name: &str, kind: &str) -> String {
    match kind {
        "class" | "struct" | "interface" | "enum" | "trait" => format!("{name}#"),
        "function" | "method" | "constructor" => format!("{name}()."),
        _ => format!("{name}."),
    }
}

/// Build a SCIP-grammar symbol. Unknown package components become `.`.
pub fn make_symbol(scheme: &str, path: &str, scope: Option<&str>, name: &str, kind: &str) -> String {
    let mut s = format!("{scheme} . . . ");
    for seg in path.split('/').filter(|x| !x.is_empty()) {
        s.push_str(seg);
        s.push('/');
    }
    if let Some(sc) = scope {
        let leaf = sc.rsplit('.').next().unwrap_or(sc);
        s.push_str(leaf);
        s.push('#');
    }
    s.push_str(&descriptor(name, kind));
    s
}

#[cfg(test)]
mod tests {

    /// `COP_PROTOCOL_FD` is the one piece of plugin behaviour that cannot be
    /// checked by reading the answers: a plugin that ignores it looks perfect on
    /// stdout. So write to a real descriptor and read the bytes back.
    ///
    /// Mutation testing found this gap. Deleting the fd arm of `open` left every
    /// test in this crate green, because none of them opened anything.
    #[cfg(unix)]
    #[test]
    fn open_writes_to_the_descriptor_the_host_selected() {
        use std::io::{Read, Seek};
        use std::os::fd::{AsRawFd, IntoRawFd};

        let dir = std::env::temp_dir().join(format!("cop-fd-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("channel");
        let file = std::fs::File::create(&path).expect("temp file");
        let fd = file.as_raw_fd();
        assert!(fd > 2, "a fresh file is never one of the standard descriptors");

        // Safety: single-threaded within this test, and the variable is read
        // immediately below by `open`.
        unsafe { std::env::set_var("COP_PROTOCOL_FD", fd.to_string()) };
        // `open` takes ownership of the descriptor, so release ours first.
        let _ = file.into_raw_fd();
        let mut ch = ProtocolChannel::open();
        let e = Engine { name: "t".into(), version: "0".into() };
        ch.send(&Answer::unsupported("x", &e, "no")).expect("write");
        drop(ch);
        unsafe { std::env::remove_var("COP_PROTOCOL_FD") };

        let mut back = String::new();
        let mut f = std::fs::File::open(&path).expect("reopen");
        f.rewind().ok();
        f.read_to_string(&mut back).expect("read");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(back.contains("\"id\":\"x\""), "nothing reached fd {fd}: {back:?}");
        assert!(back.ends_with('\n'), "COP-WIRE-COMPACT: one line, newline-terminated");
    }
    use super::*;

    #[test]
    fn symbol_grammar_matches_spec_6_2() {
        let s = make_symbol("scip-ctags", "src/app/user.py", Some("UserService"), "authenticate", "method");
        assert_eq!(s, "scip-ctags . . . src/app/user.py/UserService#authenticate().");
        assert_eq!(s.split(' ').count(), 5, "scheme manager name version descriptors");
    }

    #[test]
    fn split_handles_dotted_and_scoped_refs() {
        let mk = |n: &str| SymbolRef { name: n.into(), kind_hint: None, lang: None, context: None };
        assert_eq!(mk("greet").split(), (None, "greet"));
        assert_eq!(mk("UserService.authenticate").split(), (Some("UserService"), "authenticate"));
        assert_eq!(mk("app::user::greet").split(), (Some("user"), "greet"));
    }

    #[test]
    fn range_counts_utf16_code_units() {
        // A non-BMP character is two UTF-16 code units, per SPEC 6.3.
        let r = Range::on_line(0, 0, "𝔘ser");
        assert_eq!(r.end.character, 5);
    }

    #[test]
    fn unsupported_carries_no_confidence_or_result() {
        let e = Engine { name: "t".into(), version: "0".into() };
        let a = Answer::unsupported("1", &e, "nope");
        let v = serde_json::to_value(&a).unwrap();
        assert!(v.get("confidence").is_none(), "SPEC 5.2");
        assert!(v.get("result").is_none(), "SPEC 5.2");
        assert!(v.get("reason").is_some());
    }

    #[test]
    fn parse_line_accepts_single_and_batch() {
        // covers: COP-BATCH-ACCEPT COP-BATCH-EMPTY
        let one = r#"{"cop":"0.1","id":"a","verb":"exists","params":{}}"#;
        assert_eq!(parse_line(one).unwrap().len(), 1);
        let many = r#"[{"cop":"0.1","id":"a","verb":"exists","params":{}},
                       {"cop":"0.1","id":"b","verb":"exists","params":{}}]"#
            .replace('\n', " ");
        assert_eq!(parse_line(&many).unwrap().len(), 2);
        assert_eq!(parse_line("[]").unwrap().len(), 0, "empty batch is not an error");
    }

    #[test]
    fn answers_are_single_line() {
        // covers: COP-WIRE-COMPACT COP-WIRE-UTF8
        let e = Engine { name: "t".into(), version: "0".into() };
        let a = Answer::unsupported("1", &e, "a reason\nwith an embedded newline");
        let line = serde_json::to_string(&a).unwrap();
        assert!(!line.contains('\n'), "embedded newlines must be escaped");
        assert!(line.is_char_boundary(line.len()));
    }

    /// A shared buffer a `ProtocolChannel` can be pointed at.
    #[derive(Clone, Default)]
    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn channel_writes_one_newline_terminated_line_per_answer() {
        // covers: COP-WIRE-COMPACT COP-WIRE-CHANNEL
        let sink = Sink::default();
        let mut ch = ProtocolChannel::to_writer(sink.clone());
        let e = Engine { name: "t".into(), version: "0".into() };
        ch.send(&Answer::unsupported("1", &e, "a")).unwrap();
        ch.send(&Answer::unsupported("2", &e, "b")).unwrap();
        let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert!(out.ends_with('\n'), "each answer is newline-terminated");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "one line per answer, got: {out:?}");
        for l in lines {
            let v: serde_json::Value = serde_json::from_str(l).expect("each line is a message");
            assert!(v.get("id").is_some());
        }
    }

    #[test]
    fn protocol_fd_selection_rejects_the_standard_descriptors() {
        // covers: COP-FD-USE
        assert_eq!(ProtocolChannel::selected_fd(Some("3")), Some(3));
        assert_eq!(ProtocolChannel::selected_fd(Some("9")), Some(9));
        // 0, 1 and 2 are stdin/stdout/stderr: taking them over would be the very
        // stream corruption this mechanism exists to prevent.
        assert_eq!(ProtocolChannel::selected_fd(Some("2")), None);
        assert_eq!(ProtocolChannel::selected_fd(Some("1")), None);
        assert_eq!(ProtocolChannel::selected_fd(Some("0")), None);
        assert_eq!(ProtocolChannel::selected_fd(Some("")), None);
        assert_eq!(ProtocolChannel::selected_fd(Some("not-a-number")), None);
        assert_eq!(ProtocolChannel::selected_fd(None), None);
    }

    #[test]
    fn is_definition_survives_a_round_trip() {
        // `skip_serializing_if` must not drop a `true`, and must not invent one.
        let mk = |d: bool| Location {
            path: "a.rs".into(),
            range: Range::on_line(0, 0, "x"),
            is_definition: d,
        };
        for d in [true, false] {
            let text = serde_json::to_string(&mk(d)).unwrap();
            let back: Location = serde_json::from_str(&text).unwrap();
            assert_eq!(back.is_definition, d, "round trip lost is_definition={d}");
        }
        assert!(
            !serde_json::to_string(&mk(false)).unwrap().contains("is_definition"),
            "the default is omitted from the wire"
        );
    }

    #[test]
    fn descriptor_suffix_matches_the_symbol_kind() {
        // covers: COP-CONF-HANDSHAKE
        assert_eq!(descriptor("Foo", "class"), "Foo#");
        assert_eq!(descriptor("Foo", "interface"), "Foo#");
        assert_eq!(descriptor("bar", "method"), "bar().");
        assert_eq!(descriptor("bar", "function"), "bar().");
        assert_eq!(descriptor("BAZ", "constant"), "BAZ.");
        assert_eq!(descriptor("x", "unknown"), "x.");
    }

    #[test]
    fn parse_line_rejects_malformed_input() {
        assert!(parse_line("not json").is_err());
        assert!(parse_line(r#"{"cop":"0.1"}"#).is_err(), "a request needs id and verb");
    }

    #[test]
    fn confidence_orders_textual_below_exact() {
        assert!(Confidence::Textual < Confidence::Structural);
        assert!(Confidence::Structural < Confidence::Exact);
    }
}
