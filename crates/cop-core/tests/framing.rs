//! Property tests for the framing layer.
//!
//! A newline-delimited protocol lives or dies on framing. libFuzzer would be the
//! usual tool, but it needs a nightly toolchain; `proptest` on stable gives the
//! two properties that matter most — generation over a large input space, and
//! automatic shrinking to a minimal counter-example — and records every failing
//! seed in `.proptest-regressions`, which turns each discovery into a permanent
//! regression test.
//!
//! covers: COP-WIRE-COMPACT COP-WIRE-UTF8 COP-BATCH-ACCEPT COP-BATCH-EMPTY

use cop_core::*;
use proptest::prelude::*;

fn engine() -> Engine {
    Engine { name: "prop".into(), version: "0".into() }
}

/// Strings that actually stress a line protocol: newlines, carriage returns,
/// control characters, quotes, backslashes, and non-BMP code points.
fn nasty_string() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[\\n\\r\\t\"\\\\ ]{0,40}",
        "\\PC{0,40}",
        Just("line one\nline two".to_string()),
        Just("\r\n\r\n".to_string()),
        Just("emoji 👩‍💻 and 𝔘nicode".to_string()),
        Just("\u{0}\u{1}\u{7f}".to_string()),
        Just("\u{feff}bom".to_string()),
    ]
}

proptest! {
    /// SPEC 4.1: an answer occupies exactly one line, whatever it carries.
    /// A raw newline in a reason string would split one answer into two messages,
    /// and the second would be unparseable.
    #[test]
    fn answers_never_contain_a_raw_newline(reason in nasty_string(), id in "\\PC{1,20}") {
        let a = Answer::unsupported(&id, &engine(), reason);
        let line = serde_json::to_string(&a).unwrap();
        prop_assert!(!line.contains('\n'), "raw newline in {line:?}");
        prop_assert!(!line.contains('\r'), "raw carriage return in {line:?}");
        // And it must survive the trip back.
        let back: serde_json::Value = serde_json::from_str(&line).unwrap();
        prop_assert_eq!(back["id"].as_str().unwrap(), id);
    }

    /// Arbitrary bytes must produce an error, never a panic and never a
    /// half-parsed request. This is the property libFuzzer would look for.
    #[test]
    fn parse_line_never_panics(raw in prop::collection::vec(any::<u8>(), 0..512)) {
        let text = String::from_utf8_lossy(&raw);
        let _ = parse_line(&text); // must not panic
    }

    /// The same, with input shaped like JSON so the parser gets past the first byte.
    #[test]
    fn parse_line_never_panics_on_json_shaped_input(
        s in "\\{[\"a-z0-9:,\\[\\]{} ]{0,120}\\}"
    ) {
        let _ = parse_line(&s);
    }

    /// SPEC 4.7: a batch is exactly the concatenation of its elements. If these
    /// two paths ever disagree, a host and a plugin can hold different views of
    /// what was asked.
    #[test]
    fn a_batch_equals_its_elements(ids in prop::collection::vec("[a-z]{1,8}", 0..8)) {
        let one_at_a_time: Vec<String> = ids
            .iter()
            .map(|id| format!(r#"{{"cop":"0.1","id":"{id}","verb":"exists","params":{{}}}}"#))
            .collect();
        let batch = format!("[{}]", one_at_a_time.join(","));

        let from_batch = parse_line(&batch).expect("batch parses");
        let from_singles: Vec<Request> =
            one_at_a_time.iter().flat_map(|l| parse_line(l).expect("single parses")).collect();

        prop_assert_eq!(from_batch.len(), from_singles.len());
        for (a, b) in from_batch.iter().zip(from_singles.iter()) {
            prop_assert_eq!(&a.id, &b.id);
            prop_assert_eq!(&a.verb, &b.verb);
        }
    }

    /// SPEC 5.3: unknown fields are ignored. This is the rule that makes the
    /// protocol extensible, and the one that quietly stops holding first.
    #[test]
    fn unknown_fields_do_not_change_parsing(
        id in "[a-z]{1,8}",
        extra_key in "x_[a-z]{1,10}",
        extra_val in "[a-z0-9]{0,20}",
    ) {
        let plain = format!(r#"{{"cop":"0.1","id":"{id}","verb":"exists","params":{{}}}}"#);
        let with_extra = format!(
            r#"{{"cop":"0.1","id":"{id}","verb":"exists","params":{{}},"{extra_key}":"{extra_val}"}}"#
        );
        let a = parse_line(&plain).expect("plain parses");
        let b = parse_line(&with_extra).expect("a message from a later version still parses");
        prop_assert_eq!(a.len(), b.len());
        prop_assert_eq!(&a[0].id, &b[0].id);
        prop_assert_eq!(&a[0].verb, &b[0].verb);
    }

    /// SPEC 6.3: `character` counts UTF-16 code units. Getting this wrong puts
    /// highlights in the wrong place for every non-ASCII identifier.
    #[test]
    fn ranges_count_utf16_code_units(text in "\\PC{0,60}", line in 0u32..10_000) {
        let r = Range::on_line(line, 0, &text);
        prop_assert_eq!(r.start.line, line);
        prop_assert_eq!(r.end.line, line);
        prop_assert_eq!(r.end.character as usize, text.encode_utf16().count());
        prop_assert!(r.end.character >= r.start.character, "range runs backwards");
    }

    /// SPEC 6.2: a symbol is scheme, manager, package, version, then descriptors —
    /// five space-separated fields, whatever the names contain.
    #[test]
    fn symbols_keep_the_scip_shape(
        path in "[a-z/]{1,30}",
        name in "[A-Za-z_][A-Za-z0-9_]{0,20}",
        kind in prop::sample::select(vec!["class", "method", "function", "constant", "unknown"]),
    ) {
        let s = make_symbol("scip-test", &path, None, &name, kind);
        prop_assert_eq!(s.split(' ').count(), 5, "not five fields: {}", s);
        prop_assert!(s.starts_with("scip-test . . . "));
        prop_assert!(!s.contains('\n'));
    }
}
