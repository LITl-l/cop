# Changelog

All notable changes to the Code Oracle Protocol and its reference implementation.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Two things are versioned here and they move independently:

- **the wire protocol**, whose version is the `cop` field on every message (currently `0.1`);
- **this repository**, whose crates share one version (currently `0.3.0`).

A protocol version changes only when the wire format changes. Adding a rule, a conformance check,
or a reference plugin does not change it — that is the point of §14's additive-evolution rule, and
a repository release that bumped the wire version for a documentation fix would make every plugin
author's compatibility matrix a lie.

## [0.3.0] — unreleased

### Protocol

- **Added** `used_inputs` on `Answer` (SPEC 5.6, `COP-CACHE-REPORT`, SHOULD). A plugin reports what
  it actually read. `digest` is optional: the plugin says *what* it read and the host, which owns
  the cache, decides how to key it.
- **Added** `COP-CACHE-MISS` (SHOULD). A `not_found` answer names the index it searched rather than
  a file list. A miss keyed on files can never be invalidated by the file whose creation would fix
  it, and the same reasoning extends to any answer asserting that no alternative matched.
- **Added** `COP-CONF-FAILLIST` (SHOULD/MUST). An implementation keeps a committed list of the
  conformance cases it does not pass, and its CI runs the suite against that list.
- **Clarified** SPEC 10.1: determinism means **byte-identical** answers across runs. The previous
  wording, "modulo unspecified array order", let a plugin shuffle a two-element array and pass a
  two-run check half the time.
- **Clarified** SPEC 9.2 with an implementation note: a dispatcher that resolves the symbol before
  checking whether the verb is supported turns every unimplemented verb into `not_found`, which
  asserts an absence the plugin never established. Both reference plugins shipped this bug.

### Reference implementation

- **Added** exit-127 handling in the conformance harness (SPEC 13.2, `COP-CONF-127`): a plugin that
  declines the suite is recorded UNSUPPORTED, not failed.
- **Added** `--failure-list` to the harness, gating in both directions — an unlisted failure is a
  regression, and a listed check that passes is a stale record and fails the run.
- **Added** `--help`, `--version`, and documented exit codes to the harness CLI.
- **Added** property tests over framing, symbol grammar, and batch equivalence (proptest).
- **Added** `cargo deny` (advisories, licenses, bans, sources) to `make check`.
- **Changed** the wire schema's top level from `oneOf` to `if`/`then`/`else` dispatch on `verb` and
  `status`, so a malformed answer reports the one branch it failed instead of every branch it did
  not match.
- **Fixed** the harness had no deadline of its own while requiring one of hosts (`COP-TIME-HOST`),
  and killing a shell plugin left its children holding the pipe. Both fixed with a process group
  and `killpg`, which is what `COP-TIME-EXPIRE` asks for.

## [0.2.0]

- **Added** product classes (SPEC 2.1): every normative rule names HOST or PLUGIN. The 0.1 draft
  had rules nobody was responsible for implementing.
- **Added** stable rule identifiers `COP-<AREA>-<NAME>`, `covers:` annotations in the tree, and a
  machine-checked coverage report.
- **Added** the fd-3 protocol channel (SPEC 4.6), which makes a plugin immune to libraries that
  print to stdout.
- **Added** capability declaration and the manifest (SPEC 12).
- **Added** the hostile-plugin suite: eleven ways a plugin misbehaves, run against the host.

## [0.1.0]

- Initial protocol draft: eleven verbs, JSON Lines over stdio, SCIP symbol strings, the
  `unsupported`/`not_found` distinction, and the oracle/judge separation.
