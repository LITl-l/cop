//! cop-mischief — wrap a COP plugin and vary everything the specification leaves
//! free, deterministically from a seed.
//!
//! Let's Encrypt ships Pebble, a deliberately small ACME server that randomises
//! whatever the protocol permits, so that clients which merely happened to work
//! stop working immediately. This is the same idea for COP: a host that only
//! passes because a plugin emits fields in a convenient order has not been
//! tested, it has been lucky.
//!
//! Every transformation below is legal under the specification. If the host's
//! conclusions change when the seed changes, the host is relying on something
//! the specification does not promise.
//!
//! covers: COP-BATCH-ACCEPT
//!
//! Usage:  cop-mischief --seed 42 -- <plugin> [args...]

use serde_json::{Map, Value};
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

/// Deterministic PRNG. `rand` would be a dependency and a source of drift
/// between runs of different versions; this is reproducible forever.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn chance(&mut self, one_in: u64) -> bool {
        self.next().is_multiple_of(one_in)
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

/// Reorder object keys. JSON objects are unordered; a host that depends on the
/// order is depending on an accident.
fn shuffle_keys(v: &mut Value, rng: &mut Rng) {
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
            for (_, val) in entries.iter_mut() {
                shuffle_keys(val, rng);
            }
            rng.shuffle(&mut entries);
            *map = entries.into_iter().collect::<Map<String, Value>>();
        }
        Value::Array(items) => {
            for it in items.iter_mut() {
                shuffle_keys(it, rng);
            }
        }
        _ => {}
    }
}

/// Add fields the host has never heard of. SPEC 5.3 requires both parties to
/// ignore unknown fields; this is what makes the protocol extensible, and it is
/// the rule that quietly breaks first because nobody tests it.
fn inject_unknown(v: &mut Value, rng: &mut Rng) {
    if let Value::Object(map) = v {
        if rng.chance(2) {
            map.insert(
                format!("x_future_field_{}", rng.below(1000)),
                Value::String("a field from a later protocol version".into()),
            );
        }
        for (_, val) in map.iter_mut() {
            if rng.chance(3) {
                inject_unknown(val, rng);
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let seed = argv
        .iter()
        .position(|a| a == "--seed")
        .and_then(|i| argv.get(i + 1))
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| std::env::var("COP_SEED").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    let Some(sep) = argv.iter().position(|a| a == "--") else {
        eprintln!("usage: cop-mischief [--seed N] -- <plugin> [args...]");
        return std::process::ExitCode::from(2);
    };
    let inner = &argv[sep + 1..];
    if inner.is_empty() {
        eprintln!("cop-mischief: no plugin given");
        return std::process::ExitCode::from(2);
    }

    let mut rng = Rng::new(seed);
    eprintln!("cop-mischief: seed={seed}");

    // The wrapper reads the plugin's answers, so the plugin must speak on a pipe
    // this process owns. Passing `COP_PROTOCOL_FD` straight through would let the
    // plugin write past us to the host's descriptor, and nothing would be varied.
    let mut cmd = Command::new(&inner[0]);
    cmd.args(&inner[1..])
        .env_remove("COP_PROTOCOL_FD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = match cmd.spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cop-mischief: cannot start {}: {e}", inner[0]);
            return std::process::ExitCode::from(1);
        }
    };

    // Forward every request unchanged; the plugin is not the subject here.
    let mut sin = child.stdin.take().expect("stdin piped");
    let forward = std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if writeln!(sin, "{line}").is_err() {
                break;
            }
        }
    });

    let out = child.stdout.take().expect("stdout piped");
    // A transparent wrapper takes the child's place on the wire: it removes
    // COP_PROTOCOL_FD from the child's environment (above) and honours it itself.
    // covers: COP-FD-USE
    let mut stdout: Box<dyn Write> =
        match std::env::var("COP_PROTOCOL_FD").ok().and_then(|v| v.parse::<i32>().ok()) {
            #[cfg(unix)]
            Some(fd) if fd > 2 => {
                use std::os::fd::FromRawFd;
                Box::new(unsafe { std::fs::File::from_raw_fd(fd) })
            }
            _ => Box::new(std::io::stdout()),
        };
    let mut pending: Vec<Value> = Vec::new();

    for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut v) = serde_json::from_str::<Value>(&line) else {
            let _ = writeln!(stdout, "{line}");
            continue;
        };
        shuffle_keys(&mut v, &mut rng);
        inject_unknown(&mut v, &mut rng);

        // Hold answers back and release them out of order. SPEC 4.3 permits a
        // plugin to answer in any order; a host that assumes arrival order is
        // request order is wrong and should find out here.
        pending.push(v);
        if pending.len() >= 2 && rng.chance(2) {
            rng.shuffle(&mut pending);
            for p in pending.drain(..) {
                // Batch the release sometimes, one line at a time otherwise —
                // both are legal answer framings for a batch (SPEC 4.7).
                let _ = writeln!(stdout, "{p}");
            }
            let _ = stdout.flush();
        }
    }
    rng.shuffle(&mut pending);
    for p in pending {
        let _ = writeln!(stdout, "{p}");
    }
    let _ = stdout.flush();

    let _ = forward.join();
    let status = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
    std::process::ExitCode::from(status as u8)
}
