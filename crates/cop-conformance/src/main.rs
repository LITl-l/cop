//! cop-conformance — self-certification harness for Code Oracle Protocol plugins.
//!
//! Checks two things:
//!
//!   A. The nine conformance rules in SPEC.md section 13, which are properties of
//!      the plugin's behaviour and are checked by this runner directly.
//!   B. Fixture cases (`conformance/fixtures/*.json`), which are (request, predicate)
//!      pairs rather than golden outputs, so they do not over-constrain implementations.
//!
//! Ships as a single binary so that a plugin author in any language can run it
//! without installing a runtime they do not otherwise need.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const COP_VERSION: &str = "0.1";
/// SPEC 3.3 — these must be answerable without building the project.
const BUILD_FREE_VERBS: &[&str] = &["capabilities", "exists", "resolve", "parse_snippet"];
/// SPEC 5.4 — these may return empty evidence.
const EVIDENCE_EXEMPT: &[&str] = &["capabilities", "parse_snippet"];
/// SPEC 10.1 — a single repeat is a coin flip against a plugin that shuffles a
/// two-element list. Repeat enough that an unordered result is unlikely to survive.
const DETERMINISM_RUNS: usize = 8;

/// Parent's copy of the fd-3 write end, closed right after spawn.
static PARENT_WRITE_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

const KNOWN_VERBS: &[&str] = &[
    "resolve", "exists", "location", "signature", "value", "members", "docstring", "visibility",
    "parse_snippet", "references",
];

fn confidence_rank(c: &str) -> i32 {
    match c {
        "textual" => 0,
        "structural" => 1,
        "exact" => 2,
        _ => 9,
    }
}

// ── report ──────────────────────────────────────────────────────────────────

struct Row {
    rule: String,
    ok: bool,
    required: bool,
    detail: String,
}

#[derive(Default)]
struct Report {
    rows: Vec<Row>,
}

impl Report {
    fn add(&mut self, rule: &str, ok: bool, required: bool, detail: impl Into<String>) {
        self.rows.push(Row { rule: rule.into(), ok, required, detail: detail.into() });
    }
    fn failures(&self) -> usize {
        self.rows.iter().filter(|r| r.required && !r.ok).count()
    }
    fn render(&self) -> String {
        let w = self.rows.iter().map(|r| r.rule.len()).max().unwrap_or(10);
        self.rows
            .iter()
            .map(|r| {
                let mark = if r.ok { "PASS" } else if r.required { "FAIL" } else { "warn" };
                let mut s = format!("  [{mark}] {:<w$}", r.rule);
                if !r.detail.is_empty() {
                    s.push_str("  ");
                    s.push_str(&r.detail);
                }
                s
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── plugin driver ───────────────────────────────────────────────────────────

fn req(id: &str, verb: &str, params: Value) -> Value {
    json!({ "cop": COP_VERSION, "id": id, "verb": verb, "params": params })
}

struct Run {
    answers: Vec<Value>,
    /// Raw bytes read from the protocol channel, so that framing rules can be
    /// checked rather than assumed (a line-by-line parse hides violations).
    raw: Vec<u8>,
    code: i32,
    stderr: String,
}

/// Which channel the plugin should answer on (SPEC 4.6).
#[derive(Clone, Copy, PartialEq)]
enum Channel {
    Stdout,
    Fd3,
}

/// SPEC 4.9 COP-TIME-HOST: the host MUST enforce a wall-clock deadline. Without
/// this, a plugin that simply never answers hangs the harness forever — which the
/// hostile suite (`--hostile`, mode `hang`) demonstrates.
/// Runtime-configurable so the hostile suite can use a short deadline.
static DEADLINE_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(60);

fn plugin_deadline() -> std::time::Duration {
    std::time::Duration::from_secs(DEADLINE_SECS.load(std::sync::atomic::Ordering::Relaxed))
}

fn run_plugin(cmd: &str, workspace: &Path, requests: &[Value]) -> std::io::Result<Run> {
    run_plugin_on(cmd, workspace, requests, Channel::Stdout)
}

fn run_plugin_on(
    cmd: &str,
    workspace: &Path,
    requests: &[Value],
    channel: Channel,
) -> std::io::Result<Run> {
    let parts = shell_split(cmd);
    let (exe, args) = parts.split_first().expect("empty --plugin");
    // SPEC 4.8 COP-ENV-CLEAN: build the environment from an allowlist rather than
    // inheriting. In CI the ambient environment carries credentials.
    // covers: COP-ENV-CLEAN COP-ENV-DECLARE
    let mut command = Command::new(exe);
    command.args(args).env_clear();
    for k in ["PATH", "HOME", "LANG", "TMPDIR", "SystemRoot"] {
        if let Ok(v) = std::env::var(k) {
            command.env(k, v);
        }
    }
    // SPEC 4.6 COP-FD-OPEN: hand the plugin a dedicated protocol descriptor so
    // that stray library output on stdout cannot corrupt the stream.
    #[cfg(unix)]
    let mut fd3_read: Option<std::fs::File> = None;
    #[cfg(unix)]
    if channel == Channel::Fd3 {
        use std::os::fd::{FromRawFd, IntoRawFd};
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let (r, w) = (fds[0], fds[1]);
        fd3_read = Some(unsafe { std::fs::File::from_raw_fd(r) });
        // Keep the write end alive until after spawn, then drop it in the parent
        // so that EOF arrives when the child exits.
        let w_owned = unsafe { std::fs::File::from_raw_fd(w) };
        let w_raw = w_owned.into_raw_fd();
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(move || {
                // dup2 clears CLOEXEC on the destination, which is what makes fd 3
                // survive exec.
                if libc::dup2(w_raw, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.env("COP_PROTOCOL_FD", "3");
        // Close our copy of the write end after spawn (below).
        PARENT_WRITE_FD.store(w_raw, std::sync::atomic::Ordering::Relaxed);
    }

    // SPEC 4.9 COP-TIME-EXPIRE: put the child in its own process group so that
    // everything it spawns can be killed with it. Killing only the direct child
    // leaves grandchildren holding the pipe open, and the host still hangs — the
    // hostile suite's `hang` mode demonstrates exactly that.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .env("COP_WORKSPACE", workspace)
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    #[cfg(unix)]
    if channel == Channel::Fd3 {
        let w = PARENT_WRITE_FD.swap(-1, std::sync::atomic::Ordering::Relaxed);
        if w >= 0 {
            unsafe { libc::close(w) };
        }
    }
    {
        let mut sin = child.stdin.take().expect("stdin piped");
        for r in requests {
            writeln!(sin, "{r}")?;
        }
    } // dropping stdin closes it: SPEC 4.4 step 4

    let deadline = plugin_deadline();
    // Watchdog. Note: this kills the direct child only. A plugin that spawned its
    // own children (a language server, say) can still orphan them; SPEC 4.9
    // COP-TIME-EXPIRE asks for a process-group kill, which needs platform code.
    let killer = child.id();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if done_rx.recv_timeout(deadline).is_err() {
            #[cfg(unix)]
            unsafe {
                // Negative pid = the whole process group (setsid made pid == pgid).
                libc::kill(-(killer as i32), libc::SIGKILL);
            }
            #[cfg(not(unix))]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &killer.to_string()])
                    .status();
            }
        }
        let _ = killer; // watchdog owns the pid only for the duration of the wait
    });

    // Read fd 3 concurrently with the child's stdout/stderr, or the child blocks
    // once the pipe fills — the same class of deadlock as COP-ERR-DRAIN.
    #[cfg(unix)]
    let fd3_join = fd3_read.map(|mut f| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = f.read_to_end(&mut buf);
            buf
        })
    });

    let out = child.wait_with_output()?;
    let _ = done_tx.send(());

    #[cfg(unix)]
    let channel_bytes = match fd3_join {
        Some(h) => h.join().unwrap_or_default(),
        None => out.stdout.clone(),
    };
    #[cfg(not(unix))]
    let channel_bytes = out.stdout.clone();
    // A plugin may answer a batch either as one line per answer or as a single
    // line holding an array (SPEC 4.7); accept both.
    let mut answers: Vec<Value> = Vec::new();
    for l in String::from_utf8_lossy(&channel_bytes).lines() {
        if l.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(l) {
            Ok(Value::Array(items)) => answers.extend(items),
            Ok(v) => answers.push(v),
            Err(_) => {}
        }
    }
    Ok(Run {
        answers,
        raw: channel_bytes,
        code: out.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

fn shell_split(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut q) = (Vec::new(), String::new(), None::<char>);
    for c in s.chars() {
        match (q, c) {
            (Some(qc), c) if c == qc => q = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => q = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Content hash of a directory tree, for the "workspace unmodified" rule.
fn tree_hash(root: &Path) -> String {
    fn walk(p: &Path, acc: &mut Vec<(PathBuf, Vec<u8>)>) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            if e.is_dir() {
                walk(&e, acc);
            } else if let Ok(b) = std::fs::read(&e) {
                acc.push((e, b));
            }
        }
    }
    let mut acc = Vec::new();
    walk(root, &mut acc);
    // FNV-1a over paths and contents. Collision resistance is not a goal here;
    // detecting an accidental write is.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (p, b) in acc {
        for byte in p.to_string_lossy().as_bytes().iter().chain(b.iter()) {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
    }
    format!("{h:016x}")
}

fn dig<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = match cur {
            Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            Value::Object(_) => cur.get(seg)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn is_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Bool(b)) => !*b,
        _ => false,
    }
}

// ── fixtures ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Check {
    path: String,
    equals: Option<Value>,
    contains: Option<String>,
    non_empty: Option<bool>,
    matches: Option<String>,
}

#[derive(Deserialize)]
struct Expect {
    #[serde(default)]
    status: Vec<String>,
    /// A JSON Schema applied to the whole answer. Preferred over `require`:
    /// fixture authors get a vocabulary they already know, and the runner does
    /// not have to carry a bespoke predicate language.
    #[serde(default)]
    schema: Option<Value>,
    #[serde(default)]
    require: Vec<Check>,
    #[serde(default = "yes")]
    required: bool,
}
fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct Fixture {
    name: String,
    request: Value,
    expect: Expect,
}

// ── main ────────────────────────────────────────────────────────────────────

struct Args {
    hostile: bool,
    manifest: Option<PathBuf>,
    plugin: String,
    workspace: PathBuf,
    fixtures: Option<PathBuf>,
    schema: Option<PathBuf>,
    json_out: Option<PathBuf>,
    failure_list: Option<PathBuf>,
}

/// Usage. Printed on stdout for `--help` (a request that was answered) and on
/// stderr for a bad flag (a request that was not).
const HELP: &str = "\
cop-conformance — check a Code Oracle Protocol plugin against the specification

USAGE:
  cop-conformance --plugin <cmd> [options]

OPTIONS:
  --plugin <cmd>          Command that speaks COP on stdio. Required.
                          Split on spaces: --plugin \"bun run plugin.ts\"
  --workspace <dir>       Repository the plugin answers about  [default: .]
  --fixtures <dir>        Directory of fixture .json files to run
  --schema <file>         JSON Schema to validate every answer against
  --manifest <file>       Manifest whose declared capabilities must match
                          what the plugin reports for `capabilities`
  --failure-list <file>   Checks this implementation is known to fail, one
                          name per line, '#' comments. Gates both ways: a
                          listed check that now passes fails the run as stale
  --json <file>           Write the full report as JSON
  --hostile               Ignore --plugin's conformance and instead drive
                          deliberately broken plugins to test THIS harness
  -h, --help              Print this help
  -V, --version           Print version

EXIT STATUS:
  0   conformant (warnings may still be printed; only MUST rules gate)
  1   at least one required check failed, or the failure list is stale
  2   the harness was invoked wrongly (bad flag, missing --plugin)

Warnings are SHOULD rules. They never change the exit status, because a
specification that gates on its own advice has no advice, only rules.
";

fn parse_args() -> Args {
    let mut a = Args {
        hostile: false,
        manifest: None,
        plugin: String::new(),
        workspace: PathBuf::from("."),
        fixtures: None,
        schema: None,
        json_out: None,
        failure_list: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        match k.as_str() {
            "--hostile" => a.hostile = true,
            "--manifest" => a.manifest = Some(v().into()),
            "--plugin" => a.plugin = v(),
            "--workspace" => a.workspace = v().into(),
            "--fixtures" => a.fixtures = Some(v().into()),
            "--schema" => a.schema = Some(v().into()),
            "--json" => a.json_out = Some(v().into()),
            "--failure-list" => a.failure_list = Some(v().into()),
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("cop-conformance {} (COP {COP_VERSION})", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => {
                eprintln!("cop-conformance: unknown flag: {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }
    if a.plugin.is_empty() {
        eprintln!("--plugin is required");
        std::process::exit(2);
    }
    a
}

/// Drive deliberately non-conformant plugins and check that the HOST survives.
/// These test the host, not the plugin: every mode is a way real plugins misbehave.
fn hostile_mode(plugin: &str, workspace: &Path) -> std::process::ExitCode {
    // covers: COP-ERR-DRAIN COP-ERR-NOPARSE COP-TIME-HOST COP-TIME-EXPIRE
    // covers: COP-WIRE-MAXLINE COP-WIRE-COMPACT COP-CONF-NODEADLOCK
    let modes: &[(&str, &str)] = &[
        ("normal", "baseline: the harness itself works"),
        ("exit-immediately", "plugin dies before answering"),
        ("exit-mid-message", "truncated JSON line"),
        ("hang", "plugin never answers — host MUST time out (COP-TIME-HOST)"),
        ("wrong-id", "answer with an id nobody asked for"),
        ("duplicate-response", "two answers for one id"),
        ("pretty-printed", "multi-line JSON violates COP-WIRE-COMPACT"),
        ("huge-single-line", "20 MB line exceeds COP-WIRE-MAXLINE"),
        ("not-json", "unparseable line"),
        ("stderr-flood", "4 MB to stderr — host MUST drain concurrently (COP-ERR-DRAIN)"),
        ("stdout-pollution", "library noise on stdout"),
        ("exit-127", "plugin declines the suite — recorded UNSUPPORTED (COP-CONF-127)"),
    ];
    // The host's deadline must be shorter than the harness guard, otherwise the
    // guard fires first and we learn nothing about the host.
    DEADLINE_SECS.store(2, std::sync::atomic::Ordering::Relaxed);
    let mut rep = Report::default();
    for (mode, why) in modes {
        let cmd = format!("{plugin} {mode}");
        let started = std::time::Instant::now();
        let msgs = vec![
            req("h", "capabilities", json!({})),
            req("q1", "exists", json!({"ref": {"name": "greet"}})),
        ];
        let outcome = run_plugin_guarded(&cmd, workspace, &msgs, std::time::Duration::from_secs(10));
        let elapsed = started.elapsed();
        let (ok, detail) = match outcome {
            Ok(_) => (true, format!("survived in {:.1}s", elapsed.as_secs_f32())),
            Err(HostileFail::Timeout) => (
                false,
                format!("HOST HUNG (>10s) — {why}"),
            ),
            Err(HostileFail::Panic(e)) => (false, format!("host errored: {e}")),
        };
        rep.add(&format!("hostile/{mode}"), ok, true, detail);
    }
    println!("COP {COP_VERSION} host-robustness — {plugin}");
    println!("{}", rep.render());
    let failed = rep.failures();
    println!(
        "\n{}/{} survived{}",
        rep.rows.len() - failed,
        rep.rows.len(),
        if failed > 0 { format!("; {failed} HOST FAILURE(S)") } else { String::new() }
    );
    if failed > 0 { std::process::ExitCode::from(1) } else { std::process::ExitCode::SUCCESS }
}

enum HostileFail {
    Timeout,
    Panic(String),
}

/// `run_plugin` with a wall-clock deadline and a process-group kill.
/// Without this, a plugin that never answers hangs the harness forever — which
/// is itself a violation of COP-TIME-HOST by the harness.
fn run_plugin_guarded(
    cmd: &str,
    workspace: &Path,
    requests: &[Value],
    deadline: std::time::Duration,
) -> Result<Run, HostileFail> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (cmd, ws, reqs) = (cmd.to_string(), workspace.to_path_buf(), requests.to_vec());
    std::thread::spawn(move || {
        let _ = tx.send(run_plugin(&cmd, &ws, &reqs).map_err(|e| e.to_string()));
    });
    match rx.recv_timeout(deadline) {
        Ok(Ok(run)) => Ok(run),
        Ok(Err(e)) => Err(HostileFail::Panic(e)),
        Err(_) => Err(HostileFail::Timeout),
    }
}

fn main() -> std::process::ExitCode {
    let args = parse_args();
    if args.hostile {
        return hostile_mode(&args.plugin, &args.workspace);
    }
    let ws = &args.workspace;
    let mut rep = Report::default();
    let before = tree_hash(ws);

    // ── Rule 1: capabilities answered, and answered first ────────────────────
    let r = match run_plugin(&args.plugin, ws, &[req("h1", "capabilities", json!({}))]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to start plugin: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    // SPEC 13.2: 127 means "this implementation declines this suite", not
    // "this implementation is broken". Recording it as a failure would make
    // every new conformance case turn every existing plugin's CI red, which is
    // how conformance suites stop being adopted.
    // covers: COP-CONF-127
    if r.code == 127 && r.answers.is_empty() {
        println!("COP {COP_VERSION} conformance — {}", args.plugin);
        println!("  [skip] plugin exited 127: UNSUPPORTED, not a failure");
        return std::process::ExitCode::SUCCESS;
    }
    let Some(caps) = r.answers.first().cloned() else {
        rep.add(
            "13.1 capabilities answered",
            false,
            true,
            format!("no answer; exit={}; stderr={}", r.code, r.stderr.chars().take(120).collect::<String>()),
        );
        println!("{}", rep.render());
        return std::process::ExitCode::from(1);
    };
    // covers: COP-CONF-HANDSHAKE
rep.add(
        "13.1 capabilities answered",
        dig(&caps, "status") == Some(&json!("ok")),
        true,
        format!("status={}", dig(&caps, "status").unwrap_or(&Value::Null)),
    );
    let declared: BTreeMap<String, Value> = dig(&caps, "result.verbs")
        .and_then(|v| v.as_object())
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    rep.add(
        "13.1 declares verb map",
        !declared.is_empty(),
        true,
        format!("verbs={:?}", declared.keys().collect::<Vec<_>>()),
    );

    // ── Rule 2: one answer per request, ids echoed ───────────────────────────
    let probe: Vec<Value> = (0..3).map(|i| req(&format!("p{i}"), "capabilities", json!({}))).collect();
    let r = run_plugin(&args.plugin, ws, &probe).unwrap();
    let mut got: Vec<String> =
        r.answers.iter().filter_map(|a| a.get("id")?.as_str().map(String::from)).collect();
    got.sort();
    // covers: COP-CONF-PAIRING
rep.add(
        "13.2 one answer per request",
        got == vec!["p0", "p1", "p2"],
        true,
        format!("sent=3 got={}", got.len()),
    );

    // ── Rule 3: an unimplemented verb is `unsupported`, never `not_found` ────
    //    This is the testable half of SPEC 3.4, the load-bearing semantic.
    let undeclared: Vec<&str> = KNOWN_VERBS.iter().copied().filter(|v| !declared.contains_key(*v)).collect();
    if let Some(&v) = undeclared.first() {
        let params = match v {
            "parse_snippet" => json!({"code": "x", "lang": "python"}),
            "resolve" => json!({"ref": {"name": "Nonexistent"}}),
            _ => json!({"symbol": "scip-x . . . a/B#c()."}),
        };
        let r = run_plugin(&args.plugin, ws, &[req("u1", v, params)]).unwrap();
        let st = r.answers.first().and_then(|a| a.get("status")).cloned().unwrap_or(Value::Null);
        // covers: COP-CONF-NOTFOUND
rep.add(
            "13.3 unimplemented -> unsupported",
            st == json!("unsupported"),
            true,
            format!("verb={v} status={st}"),
        );
        rep.add("13.3 does not exit on unknown verb", r.code == 0, true, format!("exit={}", r.code));
    } else {
        rep.add("13.3 unimplemented -> unsupported", true, false, "all verbs declared; not exercised");
    }

    // ── Rule 3b: a genuinely absent symbol is `not_found`, not `unsupported` ─
    if declared.contains_key("exists") || declared.contains_key("resolve") {
        let verb = if declared.contains_key("exists") { "exists" } else { "resolve" };
        let r = run_plugin(&args.plugin, ws, &[req("n1", verb, json!({"ref": {"name": "ZzNoSuchSymbolZz"}}))])
            .unwrap();
        let a = r.answers.first().cloned().unwrap_or(Value::Null);
        let st = dig(&a, "status").cloned().unwrap_or(Value::Null);
        let ok = st == json!("not_found")
            || (st == json!("ok") && dig(&a, "result.exists") == Some(&json!(false)));
        // covers: COP-CONF-NOTFOUND
rep.add("13.3 absent symbol -> not_found", ok, true, format!("status={st}"));
    }

    // ── Rules 4, 5: confidence ceiling and evidence presence ─────────────────
    let mut batch = Vec::new();
    let mut meta = Vec::new();
    for (i, (verb, spec)) in declared.iter().enumerate() {
        if verb == "capabilities" {
            continue;
        }
        let params = match verb.as_str() {
            "exists" | "resolve" => json!({"ref": {"name": "greet"}}),
            "parse_snippet" => json!({"code": "x = 1\n", "lang": "python"}),
            _ => json!({"symbol": "scip-x . . . a/B#c()."}),
        };
        let id = format!("v{i}");
        batch.push(req(&id, verb, params));
        meta.push((id, verb.clone(), spec.clone()));
    }

    let mut last_code = 0;
    if !batch.is_empty() {
        let mut msgs = vec![req("h", "capabilities", json!({}))];
        msgs.extend(batch.iter().cloned());
        let r = run_plugin(&args.plugin, ws, &msgs).unwrap();
        last_code = r.code;
        let by_id: BTreeMap<&str, &Value> =
            r.answers.iter().filter_map(|a| Some((a.get("id")?.as_str()?, a))).collect();

        let (mut conf_ok, mut conf_why) = (true, String::new());
        let (mut ev_ok, mut ev_why) = (true, String::new());
        for (id, verb, spec) in &meta {
            let Some(a) = by_id.get(id.as_str()) else { continue };
            if let Some(c) = a.get("confidence").and_then(|c| c.as_str()) {
                let declared_c = spec.get("confidence").and_then(|c| c.as_str()).unwrap_or("textual");
                if confidence_rank(c) > confidence_rank(declared_c) {
                    conf_ok = false;
                    conf_why = format!("{verb}: answered {c} > declared {declared_c}");
                }
            }
            if a.get("status") == Some(&json!("ok"))
                && !EVIDENCE_EXEMPT.contains(&verb.as_str())
                && is_empty(a.get("evidence"))
            {
                ev_ok = false;
                ev_why = format!("{verb}: ok answer without evidence");
            }
        }
        // covers: COP-CONF-CONFIDENCE
rep.add("13.4 confidence <= declared", conf_ok, true, conf_why);
        // covers: COP-CONF-EVIDENCE
rep.add("13.5 ok answers carry evidence", ev_ok, true, ev_why);
    }

    // ── SPEC 12: the plugin ships a manifest declaring its capabilities ──────
    if let Some(mp) = &args.manifest {
        // covers: COP-CAP-DECLARE
        let text = std::fs::read_to_string(mp).unwrap_or_default();
        let doc: Result<Value, _> = serde_yaml_ng::from_str(&text);
        let caps = doc.as_ref().ok().and_then(|d| d.get("capabilities"));
        let keys = ["read", "write", "network", "exec", "env"];
        let missing: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|k| caps.and_then(|c| c.get(*k)).is_none())
            .collect();
        rep.add(
            "12 declares capabilities",
            doc.is_ok() && missing.is_empty(),
            true,
            if missing.is_empty() { String::new() } else { format!("missing: {missing:?}") },
        );
    }

    // ── SPEC 4.6: the plugin answers on fd 3 when the host opens it ──────────
    {
        // covers: COP-FD-USE COP-WIRE-CHANNEL COP-FD-OPEN
        if cfg!(unix) {
            let r =
                run_plugin_on(&args.plugin, ws, &[req("fd1", "capabilities", json!({}))], Channel::Fd3);
            let ok = r.as_ref().map(|r| r.answers.len() == 1).unwrap_or(false);
            rep.add(
                "4.6 answers on fd 3",
                ok,
                true,
                match &r {
                    Ok(r) if !ok => format!("{} answer(s) on the protocol channel", r.answers.len()),
                    Err(e) => format!("{e}"),
                    _ => String::new(),
                },
            );
        } else {
            // SPEC 4.6 lets a host omit COP_PROTOCOL_FD when it cannot pass an
            // extra handle. Reporting this as a pass would claim a guarantee the
            // platform never provided.
            rep.add(
                "4.6 answers on fd 3",
                true,
                false,
                "not exercised: this host cannot pass an extra descriptor on this platform",
            );
        }
    }

    // ── SPEC 4.7: batching ───────────────────────────────────────────────────
    {
        // covers: COP-BATCH-ACCEPT COP-CONF-BATCH
        let batch = Value::Array(
            (0..3).map(|i| req(&format!("b{i}"), "capabilities", json!({}))).collect(),
        );
        let r = run_plugin(&args.plugin, ws, std::slice::from_ref(&batch)).unwrap();
        let mut got: Vec<String> =
            r.answers.iter().filter_map(|a| a.get("id")?.as_str().map(String::from)).collect();
        got.sort();
        rep.add(
            "4.7 accepts a batch array",
            got == vec!["b0", "b1", "b2"],
            true,
            format!("sent 3 in one line, got {}", got.len()),
        );

        // covers: COP-BATCH-EMPTY
        let r = run_plugin(&args.plugin, ws, &[json!([])]).unwrap();
        rep.add(
            "4.7 empty batch is not an error",
            r.answers.is_empty() && r.code == 0,
            true,
            format!("{} answer(s), exit={}", r.answers.len(), r.code),
        );
    }

    // ── SPEC 4.1: framing hygiene, checked on raw bytes ──────────────────────
    {
        // covers: COP-WIRE-COMPACT COP-WIRE-UTF8 COP-WIRE-CLEAN COP-WIRE-STDIN
        let r = run_plugin(
            &args.plugin,
            ws,
            &[req("w1", "capabilities", json!({})), req("w2", "capabilities", json!({}))],
        )
        .unwrap();
        let utf8 = std::str::from_utf8(&r.raw);
        rep.add("4.1 protocol channel is UTF-8", utf8.is_ok(), true, "");
        if let Ok(text) = utf8 {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let all_parse = lines.iter().all(|l| serde_json::from_str::<Value>(l).is_ok());
            rep.add(
                "4.1 every channel line is a message",
                all_parse,
                true,
                if all_parse { String::new() } else { "non-protocol output on the channel".into() },
            );
            // Pretty-printed JSON spans lines, so a correct plugin emits exactly
            // one line per answer.
            rep.add(
                "4.1 one line per answer",
                lines.len() == r.answers.len(),
                true,
                format!("{} line(s) for {} answer(s)", lines.len(), r.answers.len()),
            );
        }
    }

    // ── SPEC 5.6: used_inputs (SHOULD) ───────────────────────────────────────
    if declared.contains_key("exists") {
        // covers: COP-CACHE-REPORT
        let r = run_plugin(
            &args.plugin,
            ws,
            &[req("h", "capabilities", json!({})), req("u", "exists", json!({"ref": {"name": "greet"}}))],
        )
        .unwrap();
        let reports = r
            .answers
            .iter()
            .any(|a| a.get("id") == Some(&json!("u")) && a.get("used_inputs").is_some());
        rep.add(
            "5.6 reports used_inputs",
            reports,
            false, // SHOULD, not MUST
            if reports { String::new() } else { "answers are cacheable only within one run".into() },
        );

        // A negative answer is the expensive one to cache: it depended on
        // everything searched, so it must name the index, not a file list.
        // covers: COP-CACHE-MISS
        let r = run_plugin(
            &args.plugin,
            ws,
            &[
                req("h", "capabilities", json!({})),
                req("m", "exists", json!({"ref": {"name": "cop_no_such_symbol_zz"}})),
            ],
        )
        .unwrap();
        let miss = r.answers.iter().find(|a| a.get("id") == Some(&json!("m")));
        let named_index = miss
            .filter(|a| a.get("status") == Some(&json!("not_found")))
            .and_then(|a| a.get("used_inputs"))
            .and_then(|u| u.as_array())
            .map(|u| u.iter().any(|i| i.get("kind") == Some(&json!("index"))))
            .unwrap_or(false);
        rep.add(
            "5.6 a miss names the index it searched",
            named_index,
            false, // SHOULD
            if named_index {
                String::new()
            } else {
                "a cached miss cannot be invalidated by the files that would fix it".into()
            },
        );
    }

    // ── Rule 6: core verbs are build-free ────────────────────────────────────
    let violating: Vec<&String> = declared
        .iter()
        .filter(|(v, s)| {
            BUILD_FREE_VERBS.contains(&v.as_str())
                && s.get("requires_build") == Some(&json!(true))
        })
        .map(|(v, _)| v)
        .collect();
    // covers: COP-CONF-NOBUILD
rep.add(
        "13.6 core verbs are build-free",
        violating.is_empty(),
        true,
        if violating.is_empty() { String::new() } else { format!("declared requires_build: {violating:?}") },
    );

    // ── Rule 7: exits 0 after serving ────────────────────────────────────────
    // covers: COP-CONF-EXIT
rep.add("13.7 exits 0 after serving", last_code == 0, true, format!("exit={last_code}"));

    // ── Rule 8: workspace unmodified ─────────────────────────────────────────
    // covers: COP-CONF-READONLY
rep.add("13.8 workspace unmodified", tree_hash(ws) == before, true, "");

    // ── Rule 9: determinism ──────────────────────────────────────────────────
    if !batch.is_empty() {
        let mut msgs = vec![req("h", "capabilities", json!({}))];
        msgs.extend(batch.iter().cloned());
        let strip = |answers: &[Value]| -> String {
            let v: Vec<Value> = answers
                .iter()
                .map(|a| {
                    let mut o = a.as_object().cloned().unwrap_or_default();
                    o.remove("id");
                    Value::Object(o)
                })
                .collect();
            serde_json::to_string(&v).unwrap_or_default()
        };
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..DETERMINISM_RUNS {
            seen.insert(strip(&run_plugin(&args.plugin, ws, &msgs).unwrap().answers));
        }
        // covers: COP-CONF-DETERMINISM
rep.add(
            "13.9 deterministic",
            seen.len() == 1,
            true,
            format!("{DETERMINISM_RUNS} runs, {} distinct output(s)", seen.len()),
        );
    }

    // ── Envelope schema validation ───────────────────────────────────────────
    if let Some(sp) = &args.schema {
        let schema: Value = serde_json::from_str(&std::fs::read_to_string(sp).expect("schema readable"))
            .expect("schema is JSON");
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        let mut msgs = vec![req("h", "capabilities", json!({}))];
        msgs.extend(batch.iter().cloned());
        let answers = run_plugin(&args.plugin, ws, &msgs).unwrap().answers;
        let mut bad = Vec::new();
        for m in msgs.iter().chain(answers.iter()) {
            // Report the deepest error. A conditional schema produces one error per
            // failing branch plus one at the root; the deepest is the one that names
            // the offending field, and is the only one a human can act on.
            let deepest = validator
                .iter_errors(m)
                .max_by_key(|e| e.instance_path.to_string().len());
            if let Some(e) = deepest {
                let path = e.instance_path.to_string();
                let path = if path.is_empty() { "<root>".to_string() } else { path };
                bad.push(format!(
                    "{} at {path}: {}",
                    m.get("id").unwrap_or(&Value::Null),
                    e.to_string().chars().take(90).collect::<String>()
                ));
            }
        }
        rep.add("schema: envelopes valid", bad.is_empty(), true, bad.iter().take(2).cloned().collect::<Vec<_>>().join("; "));
    }

    // ── Fixtures ─────────────────────────────────────────────────────────────
    if let Some(fx) = &args.fixtures {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(fx)
            .expect("fixtures dir readable")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();
        let cases: Vec<Fixture> = paths
            .iter()
            .map(|p| {
                serde_json::from_str(&std::fs::read_to_string(p).expect("fixture readable"))
                    .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
            })
            .collect();
        let runnable: Vec<&Fixture> = cases
            .iter()
            .filter(|c| {
                let v = c.request.get("verb").and_then(|v| v.as_str()).unwrap_or("");
                v == "capabilities" || declared.contains_key(v)
            })
            .collect();
        let skipped = cases.len() - runnable.len();

        if !runnable.is_empty() {
            let mut msgs = vec![req("h", "capabilities", json!({}))];
            for (i, c) in runnable.iter().enumerate() {
                let mut o = c.request.as_object().cloned().unwrap_or_default();
                o.insert("cop".into(), json!(COP_VERSION));
                o.insert("id".into(), json!(format!("f{i}")));
                msgs.push(Value::Object(o));
            }
            let fx_run = run_plugin(&args.plugin, ws, &msgs).unwrap();
            let declined = fx_run.code == 127;
            let answers = fx_run.answers;
            let by_id: BTreeMap<&str, &Value> =
                answers.iter().filter_map(|a| Some((a.get("id")?.as_str()?, a))).collect();

            for (i, c) in runnable.iter().enumerate() {
                let key = format!("f{i}");
                // SPEC 13.2 again, at case granularity.
                if declined && !by_id.contains_key(key.as_str()) {
                    rep.add(&format!("fixture {}", c.name), true, false, "declined (exit 127)");
                    continue;
                }
                let Some(a) = by_id.get(key.as_str()) else {
                    rep.add(&format!("fixture {}", c.name), false, true, "no answer");
                    continue;
                };
                let (mut ok, mut why) = (true, String::new());
                if let Some(sub) = &c.expect.schema {
                    match jsonschema::validator_for(sub) {
                        Err(e) => {
                            ok = false;
                            why = format!("fixture schema does not compile: {e}");
                        }
                        Ok(v) => {
                            if let Err(e) = v.validate(a) {
                                ok = false;
                                why = format!("schema: {e}");
                            }
                        }
                    }
                }
                if !c.expect.status.is_empty() {
                    let st = a.get("status").and_then(|s| s.as_str()).unwrap_or("");
                    if !c.expect.status.iter().any(|s| s == st) {
                        ok = false;
                        why = format!("status={st} not in {:?}", c.expect.status);
                    }
                }
                for chk in &c.expect.require {
                    let val = dig(a, &chk.path);
                    if let Some(want) = &chk.equals
                        && val != Some(want)
                    {
                        ok = false;
                        why = format!("{}={:?} != {want:?}", chk.path, val);
                    }
                    if let Some(sub) = &chk.contains
                        && !val.and_then(|v| v.as_str()).is_some_and(|s| s.contains(sub))
                    {
                        ok = false;
                        why = format!("{} lacks {sub:?}", chk.path);
                    }
                    if chk.non_empty == Some(true) && is_empty(val) {
                        ok = false;
                        why = format!("{} is empty", chk.path);
                    }
                    if let Some(pat) = &chk.matches {
                        // Deliberately a substring/alternation test rather than a
                        // regex dependency: fixtures should stay easy to read.
                        let s = val.map(|v| v.to_string()).unwrap_or_default();
                        let alts: Vec<&str> =
                            pat.trim_matches(|c| c == '^' || c == '$').trim_matches(|c| c == '(' || c == ')').split('|').collect();
                        if !alts.iter().any(|alt| s.contains(alt)) {
                            ok = false;
                            why = format!("{}={s} !~ {pat}", chk.path);
                        }
                    }
                }
                rep.add(&format!("fixture {}", c.name), ok, c.expect.required, why);
            }
        }
        if skipped > 0 {
            rep.add("fixtures skipped (verb not declared)", true, false, format!("{skipped} case(s)"));
        }
    }

    // SPEC 13.1: the list gates in both directions. A listed check that fails is
    // a known gap; a listed check that passes means the list is a lie about the
    // implementation, and a stale record of failures is worse than none.
    // covers: COP-CONF-FAILLIST
    let known = match &args.failure_list {
        None => BTreeSet::new(),
        Some(p) => match std::fs::read_to_string(p) {
            Ok(t) => t
                .lines()
                .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            Err(e) => {
                eprintln!("cop-conformance: cannot read --failure-list {}: {e}", p.display());
                return std::process::ExitCode::from(2);
            }
        },
    };
    let mut stale: Vec<String> = Vec::new();
    let mut excused = 0usize;
    if !known.is_empty() {
        for r in &mut rep.rows {
            if !known.contains(&r.rule) {
                continue;
            }
            if r.ok {
                stale.push(r.rule.clone());
            } else if r.required {
                r.required = false;
                r.detail = if r.detail.is_empty() {
                    "known failure".into()
                } else {
                    format!("{} [known failure]", r.detail)
                };
                excused += 1;
            }
        }
    }
    let unknown_listed: Vec<&String> =
        known.iter().filter(|k| !rep.rows.iter().any(|r| &r.rule == *k)).collect();

    println!("COP {COP_VERSION} conformance — {}", args.plugin);
    println!("{}", rep.render());
    let failed = rep.failures();
    println!(
        "\n{}/{} checks passed{}",
        rep.rows.len() - failed,
        rep.rows.len(),
        if failed > 0 { format!("; {failed} REQUIRED failure(s)") } else { "; conformant".into() }
    );

    if let Some(p) = &args.json_out {
        let rows: Vec<Value> = rep
            .rows
            .iter()
            .map(|r| json!({"rule": r.rule, "ok": r.ok, "required": r.required, "detail": r.detail}))
            .collect();
        let _ = std::fs::write(p, serde_json::to_string_pretty(&rows).unwrap());
    }

    if excused > 0 {
        println!("{excused} known failure(s) excused by the failure list");
    }
    for k in &unknown_listed {
        println!("failure list names a check this suite does not have: {k}");
    }
    for k in &stale {
        println!("STALE: '{k}' is on the failure list but passes — remove it");
    }

    if failed > 0 || !stale.is_empty() {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    }
}
