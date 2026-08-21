# Code Oracle Protocol (COP) 0.1

A wire protocol for asking questions about source code, language-agnostically.

A **host** asks; a **plugin** answers with facts and evidence. **The plugin never decides whether
anything is wrong** — all judgment stays in the host, so false-positive thresholds can be tuned
centrally without touching a single plugin.

- [`SPEC.md`](SPEC.md) — the normative specification
- [`schemas/cop-0.1.schema.json`](schemas/cop-0.1.schema.json) — machine-readable wire schema
- [`crates/cop-ctags/`](crates/cop-ctags/) — reference plugin: Rust + Universal Ctags, 135 languages, `structural`
- [`plugins/typescript/`](plugins/typescript/) — reference plugin: TypeScript + the TS type checker, `exact`
- [`crates/cop-conformance/`](crates/cop-conformance/) — self-certification harness (single binary)
- [`crates/cop-spec-check/`](crates/cop-spec-check/) — validates every example in SPEC.md against the schema
- [`docs/writing-a-plugin.md`](docs/writing-a-plugin.md) — how to write and self-certify a plugin
- [`CHANGELOG.md`](CHANGELOG.md) — what changed, and what a version number here does and does not mean

## Quick start

```bash
make check        # build, unit tests, spec examples, rule coverage, both plugins
                  # against the suite, host robustness, metamorphic, dependency audit
make mutants      # break the plugin on purpose; a survivor is a hole in the suite
make semver       # cop-core's public API against the last release
```

Talking to a plugin by hand:

```bash
printf '%s\n' \
  '{"cop":"0.1","id":"1","verb":"capabilities","params":{}}' \
  '{"cop":"0.1","id":"2","verb":"exists","params":{"ref":{"name":"greet"}}}' \
| COP_WORKSPACE=conformance/workspace target/release/cop-ctags
```

## The eleven verbs

| Verb | Tier | Build-free | Answers |
|---|---|---|---|
| `capabilities` | Required | yes | what this plugin can do |
| `resolve` | Core | yes | prose reference → canonical symbol |
| `exists` | Core | yes | is there such a thing |
| `location` | Core | — | where is it defined |
| `signature` | Core | — | params, returns, raises |
| `value` | Core | — | what is this constant set to |
| `members` | Core | — | what belongs to it (both drift directions) |
| `docstring` | Core | — | its documentation comment |
| `visibility` | Optional | — | public/private, deprecated |
| `parse_snippet` | Core | yes | does this fragment parse |
| `references` | Optional | — | who uses it |

## Why these implementation languages

The protocol is JSON Lines over stdio, so a plugin can be written in anything. These choices are
about distribution, not taste:

- **Conformance runner and reference plugin in Rust.** Every plugin author must run the suite
  regardless of what their own plugin is written in. Shipping it as a single static binary means
  they do not have to install a runtime they would not otherwise need — the same property that
  made `pre-commit` adoptable. `cop-ctags` is also a product component (the universal fallback),
  so it needs to be a binary too.
- **Second reference plugin in TypeScript.** A protocol with one implementation has not been shown
  to be language-neutral, only to be implementable once. It also reaches `exact` confidence via the
  TS type checker, which exercises a tier `cop-ctags` cannot.
- **No Python.** The one place it looked justified was a tree-sitter fallback covering 371
  languages, on the assumption that `tree-sitter-language-pack` was Python-native. It is not: that
  package is a Rust core with language bindings, and its Python wheel ships the same compiled
  Rust. Shelling out to Python would have been a detour to the same code. For a future
  tree-sitter plugin, use the `tree-sitter-language-pack` crate with `TSLP_LINK_MODE=static` for a
  curated core set and runtime download for the long tail.

## Why JSON Schema and not CUE

CUE was evaluated for the normative schema. It expresses the conditional shapes in this protocol
more naturally than JSON Schema does — but `cue def --out jsonschema` drops those very constraints
on export, silently and with exit code 0, and `cue import jsonschema` cannot read a schema that
uses `if`/`then`/`else`. Both directions of the round trip are broken at exactly the point that
matters here. CUE also has no production evaluator outside Go, so adopting it at any boundary a
plugin author touches would mean shipping a Go runtime into a Rust and TypeScript stack, and asking
every plugin author to learn it — the complaint that ended Dagger's CUE SDK.

The one idea worth taking from CUE is that assertions should be constraints rather than a bespoke
predicate language. Conformance fixtures now assert with JSON Schema subschemas, which gets that
benefit with no new dependency.

## Status

**Wire protocol `0.1`. Repository 0.3.0. Draft, unstable.** Before 1.0.0 any release may break.
Do not build a third-party plugin against 0.x expecting stability. See SPEC.md §14 for the
compatibility rules and Appendix D for what has been verified.

The two version numbers move independently: the wire version changes only when the format changes,
so adding a rule or a plugin does not invalidate a plugin author's compatibility matrix.

### What is verified

| | |
|---|---|
| spec examples validated against the schema | 22, 0 failures |
| normative rules with tests | 32 of 36 (3 pending the host, 1 a permission) |
| `cop-ctags` / `cop-typescript` conformance | 32/32 each |
| host survives deliberately broken plugins | 12/12 |
| metamorphic equivalence across seeds | verdicts identical |
| mutation score, `cop-core` + `cop-ctags` | 70/70 caught |
| dependency audit (`cargo deny`) | clean |

Each layer has caught defects in the others. The conformance suite found a `not_found`-for-
`unsupported` bug in the TypeScript plugin; the hostile suite found that the harness enforced a
timeout rule it did not itself obey; mutation testing found that the harness's own oracle test was
silently skipping, and later that a `SHOULD`-level check cannot detect a plugin which abandons the
`SHOULD` entirely.

### What is not built here

This repository is the protocol, its schema, its conformance apparatus, and two reference plugins.
The **host** — the thing that reads documentation, forms claims, asks plugins about them, and
decides what to report — is a separate program. Three rules in SPEC 12 (capability granting,
pinning, and enforcement disclosure) are marked `PENDING-HOST` in the coverage report for that
reason, rather than being counted as covered.
