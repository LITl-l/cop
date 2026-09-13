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

## Why not RDF, SPARQL, OWL, or SHACL

Wrong layer. COP is about getting a fact out of a process that does not have one yet; the semantic
web stack is about querying, inferring over, and validating a graph that already exists. The hard
part here is persuading a plugin author to answer at all — across 135 languages, without a build —
and to say so when they cannot. A query language does not supply that.

**The ontology is the Kythe failure mode again.** RDF/OWL wants a cross-language schema for symbols,
types, and members agreed up front. Kythe did that and has three indexers after a decade (§3.2).
CodeOntology did the RDF-native version — an ontology for object-oriented source, 2M triples over
OpenJDK 8, ISWC 2017 — and its parser's last commit is October 2021, at 32 stars. COP instead
carries types and signatures as strings in the language's own notation, with language-specific
richness in `extra` that the host may ignore.

**Open-world semantics collapse the distinction the protocol exists to make.** `not_found` and
`unsupported` are different answers (§3.4); under OWL they are both *not entailed* by default.
Negative facts can be asserted, but that is modelling *how far did you look* by hand, with the
reasoner contributing nothing, after paying for the ontology. OWL also has no unique name
assumption, where COP mandates SCIP symbol strings and forbids plugins inventing their own (§6.2).
For a protocol whose whole job is deciding whether this is the symbol the prose meant, that is
backwards.

**Entailment is not evidence.** Answers carry a path and range, or a description of what was
searched (§3.5), because a reviewer has to be pointed at a line. A justification over an ontology is
a different artifact, and not one that goes in a diff.

**Per-answer metadata is the awkward case for triples.** `confidence`, `evidence`, `engine`, and
`used_inputs` ride on every answer; in RDF that is per-triple annotation — reification, or RDF-star.
SPARQL 1.1 and SHACL 1.0 are Recommendations, but RDF 1.2 and the SHACL 1.2 family, the parts that
would actually carry this, are Working Drafts as of September 2026. A spec promising additive-only
changes (§14.2) should not rest on a moving draft.

**SHACL is the closest fit and the wrong distribution.** Its idea — assert with constraints rather
than a bespoke predicate language — is one this repository already took, via JSON Schema subschemas
in the conformance fixtures. But SHACL validates the shape of an RDF graph, not the correspondence
between a sentence of prose and a fact in code, which is where the host's judgment lives. Making it
normative would also ship an RDF stack to every plugin author: the objection that ended CUE above.

**SPARQL's recursion is too weak for the queries that would justify a graph.** Transitive members,
call graphs, and points-to want fixpoint rules; SPARQL has property paths. CodeQL, Doop, and Glean
all landed on Datalog.

**Where a store would genuinely fit is above the host, not below it.** Once answers accumulate
across repositories and stop being live oracle calls, putting them in a store with a query language
is reasonable — the Glean pattern §3.2 already adopts, rich facts first and cross-language views
derived on top. The trigger is the host caching answers in order to query them rather than to
memoize them, and at that point the evidence points at Datalog before SPARQL.

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
