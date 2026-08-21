//! Run the conformance suite against *this build* of cop-ctags.
//!
//! `CARGO_BIN_EXE_cop-ctags` points at the binary cargo just built, which is what
//! makes this test usable as a mutation-testing oracle: `cargo mutants` rebuilds
//! the binary with a mutation applied and asks the suite whether the result is
//! still conformant. A surviving mutant is a hole in the suite.
//!
//! `cargo mutants` copies the tree into a scratch directory, so paths relative to
//! `CARGO_MANIFEST_DIR` point at the copy, where the release harness and the
//! fixtures do not exist. `COP_SUITE_ROOT` names the real repository.
//!
//! This test **fails** rather than skipping when it cannot find the harness. An
//! oracle that quietly passes when it did not run reports every mutant as
//! surviving, which is worse than having no oracle at all — it looks like data.
//!
//! covers: COP-CONF-HANDSHAKE

use std::path::PathBuf;
use std::process::Command;

#[test]
fn this_build_is_conformant() {
    let root: PathBuf = match std::env::var("COP_SUITE_ROOT") {
        Ok(p) => PathBuf::from(p),
        Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root"),
    };
    let runner = root.join("target/release/cop-conformance");
    assert!(
        runner.exists(),
        "conformance harness not found at {}.\n\
         Build it with `cargo build --release`, and when running under \
         cargo-mutants set COP_SUITE_ROOT to the real repository root.",
        runner.display()
    );

    let out = Command::new(&runner)
        .current_dir(&root)
        .args([
            "--plugin",
            env!("CARGO_BIN_EXE_cop-ctags"),
            "--workspace",
            "conformance/workspace",
            "--fixtures",
            "conformance/fixtures",
            "--schema",
            "schemas/cop-0.1.schema.json",
            "--manifest",
            "crates/cop-ctags/cop-plugin.yaml",
        ])
        .output()
        .expect("conformance runner starts");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("checks passed"),
        "harness produced no verdict:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "cop-ctags is not conformant:\n{text}");
}

/// SPEC 4.4: `capabilities` must not pay for indexing. The cheap way to state
/// that is "build the index lazily", which is untestable — a lazy index and an
/// eager one give the same answers, only slower.
///
/// It becomes testable in the environment where the difference matters: one
/// where the backing tool is absent. A host that asks every installed plugin
/// what it can do must get an answer from a plugin whose ctags is missing,
/// rather than a dead process.
#[test]
fn capabilities_is_answered_without_ctags_on_the_path() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new(env!("CARGO_BIN_EXE_cop-ctags"))
        .env("PATH", "/nonexistent-cop-test")
        .env("COP_WORKSPACE", ".")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("plugin starts");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"{\"cop\":\"0.1\",\"id\":\"c\",\"verb\":\"capabilities\",\"params\":{}}\n")
        .expect("write");
    let out = child.wait_with_output().expect("plugin exits");

    assert!(out.status.success(), "exit code describes the plugin, not the tool: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("\"id\":\"c\""), "no answer to capabilities: {text:?}");
    assert!(text.contains("\"status\":\"ok\""), "capabilities must not fail with no ctags: {text:?}");
}
