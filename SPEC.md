# Code Oracle Protocol (COP)

**Wire version:** `0.1` — the value of the `cop` field on every message.
**Document version:** 0.3.0 (draft — unstable, see §14)
**Status:** Draft for review. Not yet suitable for third-party plugin authors to depend on.
**Date:** 2026-08-21

---

## 1. Overview

COP is a wire protocol for asking questions about source code, language-agnostically.

A **host** (a tool that needs facts about code) sends requests to a **plugin** (a program that
knows one or more languages). The plugin answers with facts and evidence. **The plugin never
decides whether anything is wrong.**

COP exists because doc/code consistency checking, and code-aware agent tooling generally, need a
small set of code facts across many languages — and no such protocol exists. LSP is the closest,
but it is stateful, interactive, and has no bulk-enumeration verb. SCIP is a static index format,
not a query protocol, and answers only definitions and references.

### 1.1 What COP is not

- Not a code-analysis framework. It defines a wire format, not an implementation.
- Not a linter protocol. Plugins return facts, not diagnostics. Compare SARIF or reviewdog's
  RDFormat, which are result formats — COP sits upstream of those.
- Not a universal AST or type system. See §3.2.

---

## 2. Conformance language

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT, RECOMMENDED, MAY,
and OPTIONAL in this document are to be interpreted as described in RFC 2119 and RFC 8174.

- **Host** — the program that sends requests and interprets answers.
- **Plugin** — the program that receives requests and answers them.
- **Verb** — a named operation (§7).
- **Answer** — a response to a verb request.

### 2.1 Classes of products

Every normative rule in this specification applies to exactly one **class**: `HOST` or `PLUGIN`.
A rule that does not name its class is a defect in this document.

Rules carry stable identifiers of the form `COP-<AREA>-<NAME>`, listed in §15 and cross-referenced
from the conformance suite. Identifiers are permanent: a rule that is withdrawn keeps its
identifier and is marked withdrawn.

*Rationale.* The W3C QA Framework requires a specification to say what conforms to it. Without
product classes, "the protocol MUST reject oversized messages" is unimplementable — nobody knows
whose job it is. This document's 0.1 draft had that defect throughout.

---

## 3. Design principles

These are normative in spirit; every rule in this spec traces back to one of them.

### 3.1 Oracles, not judges

A plugin answers questions. It MUST NOT assign severity, decide whether to report, or express
results through its exit code. All judgment lives in the host.

*Rationale.* Plugin architectures where the plugin renders verdicts (Danger JS: plugins call
`fail()`; pre-commit: the contract is an exit code and stdout) cannot do cross-plugin correlation,
deduplication, suppression, baselines, or central threshold tuning. When false positives become a
problem — and for doc checking they will — the host must be able to fix it without touching a
single plugin.

### 3.2 The protocol does not model semantics

COP carries types, signatures, and values as **strings in the language's own notation**. It does
not define a cross-language type system, a universal AST, or a unified symbol taxonomy beyond the
naming scheme in §6.2.

*Rationale.* Kythe defined a rich cross-language graph schema (≈25 node kinds, 60+ edge kinds) and
after more than a decade has indexers for three languages; it is now in maintenance mode. LSP
deliberately declined to standardize ASTs, modeling only document URIs and text positions, and won.
Glean's approach is the refinement: accept rich language-specific facts first, and define
cross-language views *on top* as derived predicates. COP follows Glean — the `extra` field (§5.3)
is where language-specific richness goes, and the host may ignore it entirely.

### 3.3 No build requirement for the core verbs

`capabilities`, `exists`, `resolve`, and `parse_snippet` MUST be answerable without building the
project. Verbs that need a build MUST be declared as such (§8) and the host MUST be able to run in
a no-build mode.

*Rationale.* Kythe's extractors require integration with the build system — you cannot drop it into
an arbitrary repository and run it. This is the single most-cited reason it did not spread.

### 3.4 Absence of knowledge is not evidence of absence

`unsupported` and `not_found` are different answers and MUST NOT be conflated (§6.5).

*Rationale.* This is the highest-leverage semantic in the protocol. A checker that treats "I could
not analyze this" as "the documentation is wrong" produces false positives at the rate of its own
coverage gaps, and gets disabled within a week.

### 3.5 Every answer carries evidence

Answers MUST carry either locations (positive answers) or a description of what was searched
(negative answers). An answer without evidence is not actionable by a human reviewer.

### 3.6 Partial implementation is a first-class participation mode

A plugin implementing only `capabilities` and `exists` is conformant and useful. The host degrades
gracefully via capability negotiation.

*Rationale.* asdf's ecosystem grew from a three-shell-script contract. stack-graphs was
architecturally sound and language-agnostic but required authors to write complex per-language
`.tsg` grammars; it was archived in September 2025 with few languages supported. Contract size is
the binding constraint on ecosystem growth.

---

## 4. Transport

### 4.1 Framing

COP is **JSON Lines over a byte stream**. The host starts the plugin as a child process.

- `COP-WIRE-STDIN` (PLUGIN) — The plugin MUST read requests from **stdin**, one JSON object per
  line, terminated by `\n` (U+000A).
- `COP-WIRE-CHANNEL` (PLUGIN) — The plugin MUST write answers to the **protocol channel**: file
  descriptor 3 when the host has opened it, otherwise stdout. The host signals which by setting
  `COP_PROTOCOL_FD` (§4.6).
- `COP-WIRE-COMPACT` (PLUGIN) — Each answer MUST occupy exactly one line. The plugin MUST NOT
  pretty-print. A raw newline MUST NOT appear inside the JSON; encode as `\n`.
- `COP-WIRE-UTF8` (PLUGIN) — Encoding MUST be UTF-8. A byte-order mark MUST NOT be emitted.
- `COP-WIRE-CLEAN` (PLUGIN) — The plugin MUST NOT write anything other than protocol messages to
  the protocol channel.
- `COP-WIRE-MAXLINE` (HOST) — A host MUST NOT buffer a line without bound. Neither party is
  required to accept a line longer than **8 MiB**; a host receiving a longer one MUST treat the
  plugin as faulted (§9.4).

*Rationale.* These rules are taken from the Model Context Protocol's stdio transport, which
reached the same design independently and is the largest new plugin ecosystem to do so. The
single most common practical failure of newline-delimited protocols is a library the plugin
depends on printing a warning to stdout and corrupting the stream — hence fd 3 (§4.6).

### 4.2 stderr

- `COP-ERR-FREE` (PLUGIN) — The plugin MAY write anything to **stderr**.
- `COP-ERR-NOPARSE` (HOST) — The host MUST NOT parse stderr as protocol data, and MUST NOT treat
  output on stderr as an indication of error. The host SHOULD capture it for diagnostics.
- `COP-ERR-DRAIN` (HOST) — The host MUST drain stderr concurrently with the protocol channel.

*Rationale for `COP-ERR-DRAIN`.* A host that reads the protocol channel to completion before
reading stderr deadlocks the moment a plugin fills the stderr pipe buffer — 64 KiB on Linux. This
is the single most common bug in subprocess-driving code and is almost never tested. The
conformance suite exercises it (§13).

### 4.3 Pipelining and ordering

The host MAY send multiple requests before reading any answer. The plugin MAY answer out of order.
Every answer MUST echo the request's `id`. The plugin MUST eventually emit exactly one answer per
request it has read.

### 4.4 Lifecycle

1. Host starts the plugin process.
2. Host sends a `capabilities` request. The plugin MUST answer it before any other verb is sent.
3. Host sends zero or more verb requests.
4. Host closes the plugin's stdin.
5. Plugin flushes remaining answers and exits.

The plugin SHOULD remain alive across many requests so that startup cost is amortized. The plugin
MUST NOT require that requests arrive in any particular order after the handshake.

### 4.6 The protocol channel

`COP-FD-OPEN` (HOST) — The host SHOULD open file descriptor 3 for the plugin and set the
environment variable `COP_PROTOCOL_FD=3`. When it does not, it MUST NOT set `COP_PROTOCOL_FD`, and
the plugin falls back to stdout.

`COP-FD-USE` (PLUGIN) — A plugin MUST honour `COP_PROTOCOL_FD` when present.

This exists so that a library the plugin links against can print to stdout without corrupting the
protocol. Plugins that wrap existing native tools cannot always prevent that output; separating
the channels removes the failure mode instead of asking every plugin author to suppress it.
Windows hosts that cannot pass an extra handle simply omit `COP_PROTOCOL_FD`.

### 4.7 Batching

`COP-BATCH-ACCEPT` (PLUGIN) — A plugin MUST accept a line containing a JSON **array** of request
objects, and MUST answer each element. It MAY answer with one line per answer, or with a single
line containing an array of answers.

`COP-BATCH-EMPTY` (PLUGIN) — An empty array MUST produce no answers and MUST NOT be an error.

A pull request produces hundreds to thousands of queries. Amortising the read/write round trip
across a batch is a larger performance lever than any choice of serialization format, which is why
this is mandatory rather than optional.

### 4.8 Environment

`COP-ENV-CLEAN` (HOST) — The host MUST NOT pass its own environment to a plugin. It MUST construct
the plugin's environment from an allowlist, and that allowlist MUST NOT include variables carrying
credentials by default.

`COP-ENV-DECLARE` (PLUGIN) — A plugin MUST declare the environment variables it needs in its
manifest (§12). A variable not declared is not passed.

*Rationale.* In CI the ambient environment carries `GITHUB_TOKEN`, `NPM_TOKEN`, `AWS_*` and
similar. The 2025 `tj-actions/changed-files` compromise exfiltrated exactly these by dumping
runner memory. Passing a clean environment is the cheapest meaningful mitigation available to this
protocol, and making it the default cannot be done later without breaking every plugin that came
to rely on ambient variables.

### 4.9 Timeouts

`COP-TIME-HOST` (HOST) — The host MUST enforce a wall-clock deadline on every request and on the
plugin process as a whole. The default per-request deadline SHOULD be 5 s interactively and 60 s
in batch.

`COP-TIME-EXPIRE` (HOST) — On expiry the host MUST treat the outstanding request as `unsupported`,
never as `not_found` (§3.4), and SHOULD terminate the plugin's **process group**, not just the
plugin process.

A liveness requirement without a number is not testable, and two implementations will diverge on
it. Killing only the direct child orphans whatever the plugin spawned — a plugin wrapping a
language server is exactly that case.

### 4.5 Exit code

The plugin's exit code describes **whether the plugin itself worked**, never whether any answer was
negative.

| Code | Meaning |
|---|---|
| `0` | Plugin ran and answered every request it read. Answers MAY include `not_found`, `unsupported`, or per-request `error`. |
| `1` | Plugin failed to start or crashed. Any answers already emitted remain valid. |
| `2` | Protocol violation detected by the plugin (unparseable input, unsupported protocol version). |

The host MUST treat a non-zero exit as a plugin fault, not as a finding about the code.

*Rationale.* Exercism's test-runner contract requires exit 0 regardless of test outcome,
separating "the runner worked" from "the tests passed". Conflating the two makes plugin breakage
indistinguishable from a real finding.

---

## 5. Envelope

### 5.1 Request

```json
{
  "cop": "0.1",
  "id": "r1",
  "verb": "resolve",
  "params": { "ref": { "name": "UserService.authenticate", "lang": "python" } }
}
```

| Field | Type | Req. | Description |
|---|---|---|---|
| `cop` | string | MUST | Protocol version, `MAJOR.MINOR`. See §14. |
| `id` | string | MUST | Opaque correlation ID, unique within the process lifetime. |
| `verb` | string | MUST | Verb name (§7). |
| `params` | object | MUST | Verb-specific parameters. MAY be `{}`. |
| `deadline_ms` | integer | MAY | Advisory budget for this request. The host still enforces its own timeout (§11). |

### 5.2 Answer

```json
{
  "cop": "0.1",
  "id": "r1",
  "status": "ok",
  "confidence": "exact",
  "result": { },
  "evidence": [ ],
  "engine": { "name": "cop-python-scip", "version": "0.3.1" }
}
```

| Field | Type | Req. | Description |
|---|---|---|---|
| `cop` | string | MUST | Protocol version the plugin is answering with. |
| `id` | string | MUST | Echo of the request `id`. |
| `status` | string | MUST | One of §6.5. |
| `confidence` | string | MUST unless `status` is `unsupported` or `error` | One of §6.4. |
| `result` | object | MUST when `status` is `ok`; MUST be absent otherwise | Verb-specific payload. |
| `evidence` | array | MUST when `status` is `ok` (§5.4) | Locations supporting the answer. |
| `searched` | object | SHOULD when `status` is `not_found` | What was searched (§5.5). |
| `candidates` | array | MUST when `status` is `ambiguous` | Competing answers (§7.2). |
| `engine` | object | MUST | `{name, version}` of the analyzer that produced this answer. |
| `reason` | string | MUST when `status` is `unsupported` or `error` | Human-readable, one line. |
| `used_inputs` | array | SHOULD | What the plugin actually read to produce this answer (§5.6). |
| `extra` | object | MAY | Language-specific data. The host MUST ignore fields it does not recognize. |

### 5.3 Must-ignore rule

Both parties MUST ignore unknown object fields rather than erroring. This is what makes additive
protocol evolution possible (§14).

### 5.4 Evidence

`evidence` is an array of `Location` (§6.3). For `status: "ok"` it MUST be non-empty, **except**
for verbs whose result is inherently non-positional (`capabilities`, `parse_snippet`), which MAY
return `[]`.

### 5.5 Searched

For `status: "not_found"`, `searched` describes the scope that was examined, so that a human can
judge whether the negative answer is trustworthy.

```json
{
  "searched": {
    "scope": "index",
    "roots": ["src/"],
    "file_count": 412,
    "index_commit": "9f2c1ab",
    "note": "SCIP index built at 9f2c1ab; files changed since are not reflected"
  }
}
```

All fields are OPTIONAL. `scope` SHOULD be one of `index`, `files`, `package`, `workspace`.

### 5.6 Used inputs

`COP-CACHE-REPORT` (PLUGIN) — A plugin SHOULD report the inputs it read while producing an answer.

```json
{
  "used_inputs": [
    { "kind": "file", "path": "src/app/config.py", "digest": "blake3:9f2c1ab…" },
    { "kind": "file", "path": "pyproject.toml", "digest": "blake3:4d81ee…" },
    { "kind": "env",  "name": "PYTHONPATH" },
    { "kind": "tool", "name": "scip-python", "version": "0.6.2" }
  ]
}
```

`kind` is an open enum; `file`, `env`, `tool`, and `index` are defined. `digest` is OPTIONAL: a
plugin that reports a path without one is saying *what* it read and leaving *how to key it* to the
host, which is where the cache lives and where the choice of hash belongs.

A plugin that cannot track its reads MAY return an empty array, and the host then treats every
answer from that plugin as uncacheable beyond the current run.

`COP-CACHE-MISS` (PLUGIN) — An answer of `not_found` depends on everything that was searched,
not on a single file, and SHOULD report `{"kind": "index", ...}` naming the scope. This makes
negative answers cheap to produce and expensive to keep: a miss cached against a file list can
never be invalidated by the file whose creation would fix it.

The same reasoning extends to any answer that asserts the absence of alternatives. `resolve`
returning a single symbol claims nothing else matched, so a plugin SHOULD report the index there
too, alongside the files behind its evidence. `exists` returning true does not: no new file can
make an existing symbol stop existing.

*Rationale.* The host caches answers, and a cache is only as correct as its knowledge of what an
answer depended on. **Observation beats declaration**: a manifest field where the plugin promises
what it will read is wrong the first time the plugin's behaviour changes and nobody updates it.
Reporting what was actually read makes the reverse index — which claims a diff invalidates —
derivable rather than separately maintained.

This field exists in 0.2 with a `SHOULD` because adding it later would require every plugin to be
rewritten. A protocol can gain optional fields; it cannot gain load-bearing ones.

---

## 6. Data types

### 6.1 SymbolRef — an imprecise reference, as written in prose

What the documentation said. The host constructs these; plugins consume them.

```json
{
  "name": "UserService.authenticate",
  "kind_hint": "method",
  "lang": "python",
  "context": {
    "path": "docs/api.md",
    "range": { "start": {"line": 42, "character": 4}, "end": {"line": 42, "character": 28} },
    "imports": ["from app.services import UserService"],
    "near_paths": ["src/app/services/user.py"]
  }
}
```

| Field | Type | Req. | Description |
|---|---|---|---|
| `name` | string | MUST | The reference as written. Dotted, `::`-separated, or bare — plugins parse per their language. |
| `kind_hint` | string | MAY | One of §6.6. Advisory only. |
| `lang` | string | MAY | Language identifier (§6.7). Absent means the host does not know. |
| `context` | object | MAY | Where the reference appeared and what surrounds it. All subfields OPTIONAL. |

A plugin MUST NOT require `kind_hint`, `lang`, or `context` to answer. They exist to narrow
ambiguity, not to gate it.

### 6.2 SymbolId — a canonical identifier

COP adopts **SCIP's symbol string grammar** verbatim. Plugins MUST NOT invent their own
identifier scheme.

```
<symbol>         ::= <scheme> ' ' <package> ' ' (<descriptor>)+ | 'local ' <local-id>
<package>        ::= <manager> ' ' <package-name> ' ' <version>
<descriptor>     ::= <namespace> | <type> | <term> | <method>
                   | <type-parameter> | <parameter> | <meta> | <macro>
<namespace>      ::= <name> '/'
<type>           ::= <name> '#'
<term>           ::= <name> '.'
<method>         ::= <name> '(' (<method-disambiguator>)? ').'
<type-parameter> ::= '[' <name> ']'
<parameter>      ::= '(' <name> ')'
<meta>           ::= <name> ':'
<macro>          ::= <name> '!'
```

Example:

```
scip-python python myproject 1.2.0 app/services/UserService#authenticate().
```

Rules:

- A SymbolId MUST be stable across runs given the same source at the same revision (§10).
- `local <id>` SymbolIds are scoped to one file and one plugin invocation. The host MUST NOT
  persist them, use them as cache keys, or send them back in a later process.
- Where a package manager or version is unknown, the plugin SHOULD emit `.` for that component
  rather than omitting it, keeping the grammar well-formed.

*Rationale.* SCIP replaced LSIF partly because LSIF used opaque integer IDs, which made payloads
undebuggable and imposed ordering constraints that blocked incremental updates. Human-readable
string symbols avoid both. Reusing the grammar also means SCIP indexers can back a COP plugin with
a thin adapter.

### 6.3 Location and Range

```json
{
  "path": "src/app/services/user.py",
  "range": {
    "start": { "line": 88, "character": 4 },
    "end":   { "line": 88, "character": 16 }
  },
  "is_definition": true
}
```

- `path` MUST be relative to the workspace root, using `/` separators, with no leading `./`.
- `line` and `character` are **zero-based**. `character` counts **UTF-16 code units**, matching LSP.
- `end` is **exclusive**.
- `is_definition` is OPTIONAL, default `false`.

*Note.* SCIP uses UTF-8 byte offsets in some contexts; a SCIP-backed plugin MUST convert. This
spec chooses LSP's convention because hosts more often interoperate with editors than with indexes.

### 6.4 Confidence

Confidence describes **the mechanism that produced the answer**, not a probability.

| Value | Meaning | Typical producer |
|---|---|---|
| `exact` | Type-aware name resolution. As reliable as the language's own tooling. | compiler, type checker, SCIP index, LSP server |
| `structural` | Syntax-tree based, scope-unaware. Same-named symbols in different scopes may be conflated. | tree-sitter `tags.scm`, ctags |
| `textual` | Token or regex match. Establishes presence of a string, nothing more. | grep, generic-mode matcher |

A plugin MUST report the confidence of the weakest mechanism that contributed to the answer.

Hosts SHOULD map confidence to gate severity, and this spec RECOMMENDS:

| Confidence | Recommended maximum severity |
|---|---|
| `exact` | blocking |
| `structural` | warning |
| `textual` | informational |

A host MUST NOT raise a `structural` or `textual` answer to blocking severity without an explicit
operator opt-in.

### 6.5 Status

| Value | Meaning | Is it evidence about the code? |
|---|---|---|
| `ok` | The question was answered. | Yes. |
| `not_found` | The plugin searched and the subject does not exist. | **Yes — a positive claim of absence.** |
| `ambiguous` | Multiple candidates matched; none was selected. | Partially; see §7.2. |
| `unsupported` | The plugin cannot answer this verb, language, or construct. | **No. Carries zero information.** |
| `error` | The plugin failed on this request. | No. |

The host MUST treat `unsupported` as *unverified* and MUST NOT derive a finding from it. The host
SHOULD count and report unverified claims separately, as coverage.

The distinction between `not_found` and `unsupported` is the load-bearing semantic of this
protocol. A plugin that returns `not_found` when it means "I did not look" is non-conformant.

### 6.6 SymbolKind

`namespace`, `package`, `module`, `class`, `interface`, `trait`, `struct`, `enum`, `enum_member`,
`type_alias`, `function`, `method`, `constructor`, `property`, `field`, `variable`, `constant`,
`macro`, `type_parameter`, `parameter`, `unknown`.

Plugins MAY return `unknown`. Hosts MUST accept unrecognized values (§5.3) and treat them as
`unknown`.

### 6.7 Language identifiers

Language identifiers are lowercase strings. Where an identifier exists in the LSP language
identifier list, plugins MUST use it (`python`, `typescript`, `csharp`, `objective-c`, ...).
Otherwise, plugins SHOULD use the lowercase canonical name of the language.

---

## 7. Verbs

Every verb is listed with its `params` schema and its `result` schema (the payload under
`result` when `status` is `ok`).

Priority tiers, referenced in §8:

- **Required** — a conformant plugin MUST implement.
- **Core** — a plugin SHOULD implement; most host checks depend on these.
- **Optional** — a plugin MAY implement.

---

### 7.1 `capabilities` — Required

Announces what the plugin can do. The host MUST send this first (§4.4).

**params:** `{}`

**result:**

```json
{
  "cop_versions": ["0.1"],
  "engine": { "name": "cop-python-scip", "version": "0.3.1" },
  "languages": ["python"],
  "file_patterns": ["**/*.py", "**/*.pyi"],
  "verbs": {
    "resolve":       { "confidence": "exact",      "requires_build": false },
    "exists":        { "confidence": "exact",      "requires_build": false },
    "location":      { "confidence": "exact",      "requires_build": false },
    "signature":     { "confidence": "exact",      "requires_build": true  },
    "value":         { "confidence": "structural", "requires_build": false },
    "members":       { "confidence": "exact",      "requires_build": true  },
    "docstring":     { "confidence": "exact",      "requires_build": false },
    "parse_snippet": { "confidence": "exact",      "requires_build": false }
  },
  "cache_keys": ["pyproject.toml", "setup.cfg", "requirements*.txt"],
  "limits": { "suggested_timeout_ms": 30000, "max_batch": 256 }
}
```

| Field | Req. | Description |
|---|---|---|
| `cop_versions` | MUST | Protocol versions supported, most preferred first. |
| `engine` | MUST | Identifies the analyzer. |
| `languages` | MUST | Language identifiers (§6.7) this plugin answers for. |
| `file_patterns` | SHOULD | Globs the plugin claims. Used for routing. |
| `verbs` | MUST | Map of verb name → `{confidence, requires_build}`. **A verb absent from this map MUST be treated by the host as unimplemented**, and the host MUST NOT send it. |
| `cache_keys` | SHOULD | Files whose contents invalidate this plugin's answers (§10.2). |
| `limits` | MAY | Advisory. The host's own limits win (§11). |

`confidence` here is the **best** confidence the plugin can produce for that verb. Individual
answers MAY report lower confidence; they MUST NOT report higher.

If the host requests a protocol version absent from `cop_versions`, the plugin MUST exit with
code 2 after emitting an `error` answer.

---

### 7.2 `resolve` — Core

Turn a prose reference into canonical SymbolIds. This is the entry point for most other verbs.

**params:**

```json
{ "ref": { "name": "UserService.authenticate", "kind_hint": "method", "lang": "python" } }
```

**result:**

```json
{
  "symbol": "scip-python python myproject 1.2.0 app/services/UserService#authenticate().",
  "kind": "method"
}
```

**Ambiguity.** When more than one symbol matches and the plugin cannot choose, it MUST answer
`status: "ambiguous"` with a `candidates` array, ordered by descending plausibility:

```json
{
  "cop": "0.1", "id": "r7", "status": "ambiguous", "confidence": "structural",
  "candidates": [
    { "symbol": "scip-python python myproject 1.2.0 app/services/UserService#authenticate().",
      "kind": "method",
      "evidence": [{"path": "src/app/services/user.py", "range": {"start":{"line":88,"character":4},"end":{"line":88,"character":16}}, "is_definition": true}] },
    { "symbol": "scip-python python myproject 1.2.0 tests/fakes/UserService#authenticate().",
      "kind": "method",
      "evidence": [{"path": "tests/fakes/user.py", "range": {"start":{"line":12,"character":4},"end":{"line":12,"character":16}}, "is_definition": true}] }
  ],
  "engine": { "name": "cop-python-ctags", "version": "0.1.0" }
}
```

The host decides what to do with ambiguity. It SHOULD NOT report a finding based on an ambiguous
resolution alone.

**Relationship to `exists`.** A plugin implementing `resolve` need not implement `exists`; the
host derives existence from `ok` versus `not_found`. A plugin MAY implement `exists` alone.

---

### 7.3 `exists` — Core

Cheapest possible question: is there such a thing?

**params:** `{"ref": {"name": "greet", "lang": "python"}}` **or** `{"symbol": "scip-python python myproject 1.2.0 app/greeting/greet()."}`

**result:**

```json
{ "exists": true, "kind": "method" }
```

`status: "ok"` with `result.exists: false` and `status: "not_found"` are equivalent; plugins
SHOULD use `not_found` and hosts MUST accept both.

This verb exists so that a plugin can be useful with roughly thirty lines of adapter code. See
Appendix A.

---

### 7.4 `location` — Core

**params:** `{"symbol": "scip-python python myproject 1.2.0 app/greeting/greet().", "definitions_only": true}`

`definitions_only` is OPTIONAL, default `true`.

**result:**

```json
{ "locations": [ { "path": "src/app/services/user.py", "range": { "start": {"line":88,"character":4}, "end": {"line":88,"character":16} }, "is_definition": true } ] }
```

---

### 7.5 `signature` — Core

**params:** `{"symbol": "scip-python python myproject 1.2.0 app/greeting/greet()."}`

**result:**

```json
{
  "render": "def authenticate(self, token: str, *, max_age: int = 3600) -> User",
  "params": [
    { "name": "self",    "kind": "positional", "optional": false, "variadic": false },
    { "name": "token",   "kind": "positional", "type": "str", "optional": false, "variadic": false },
    { "name": "max_age", "kind": "keyword", "type": "int", "default": "3600", "optional": true, "variadic": false }
  ],
  "returns": { "type": "User" },
  "raises": [ { "type": "TokenExpired" } ],
  "type_params": [],
  "modifiers": ["async"]
}
```

| Field | Req. | Notes |
|---|---|---|
| `render` | MUST | One-line human-readable signature in the language's own syntax. Used verbatim in host messages. |
| `params` | MUST | Ordered. `kind` ∈ `positional`, `keyword`, `both`, `receiver`. |
| `returns` | MAY | Absent means "no return type information", not "returns nothing". Use `{"type": "void"}` or the language's equivalent for that. |
| `raises` | MAY | Declared or statically inferable error types. Absent ≠ cannot throw. |
| `type_params` | MAY | Generic parameters. |
| `modifiers` | MAY | Free-form strings: `async`, `static`, `abstract`, `const`, `unsafe`, ... |

**`type` fields are strings in the language's own notation.** COP does not normalize them.
Comparing a documented type against a code type is the host's problem, and the host is expected to
do it fuzzily.

---

### 7.6 `value` — Core

Answers "what is this constant actually set to?" — the verb behind claims like *"the default
timeout is 30 seconds"*.

**params:** `{"symbol": "scip-python python myproject 1.2.0 app/greeting/greet()."}`

**result:**

```json
{
  "repr": "30",
  "json": 30,
  "type": "int",
  "evaluation": "literal"
}
```

| Field | Req. | Notes |
|---|---|---|
| `repr` | MUST | The value as written in source. |
| `json` | MAY | The value as JSON, when representable. Absent when not. |
| `type` | MAY | Language notation. |
| `evaluation` | MUST | `literal`, `const_folded`, `computed`, or `unknown`. |

`evaluation: "computed"` means the plugin evaluated an expression whose result may depend on
environment. The host SHOULD NOT block on `computed`.

**Units are not modeled.** Source says `30`; prose says "30 seconds". Reconciling that is
judgment and belongs to the host.

---

### 7.7 `members` — Core

Enumerate what belongs to a symbol. This is the only verb that lets a host detect **both**
directions of drift — documentation listing something that no longer exists, and documentation
missing something that now does.

**params:**

```json
{
  "symbol": "scip-python python myproject 1.2.0 app/config/Format#",
  "kinds": ["enum_member"],
  "include_inherited": false,
  "include_private": false
}
```

All fields except `symbol` are OPTIONAL. Defaults: `kinds` unset (all), `include_inherited` false,
`include_private` false.

**result:**

```json
{
  "members": [
    { "symbol": "scip-python python myproject 1.2.0 app/config/Format#JSON.", "name": "JSON", "kind": "enum_member" },
    { "symbol": "scip-python python myproject 1.2.0 app/config/Format#YAML.", "name": "YAML", "kind": "enum_member" },
    { "symbol": "scip-python python myproject 1.2.0 app/config/Format#TOML.", "name": "TOML", "kind": "enum_member" }
  ],
  "complete": true
}
```

`complete` MUST be `true` only when the plugin is confident the list is exhaustive. When
`complete` is `false`, the host MUST NOT report "documentation is missing X" findings from it.

---

### 7.8 `docstring` — Core

**params:** `{"symbol": "scip-python python myproject 1.2.0 app/greeting/greet()."}`

**result:**

```json
{ "text": "Authenticate a bearer token and return the owning user.", "format": "plaintext" }
```

`format` ∈ `plaintext`, `markdown`, `rst`, `javadoc`, `xmldoc`, `unknown`.

---

### 7.9 `visibility` — Optional

**params:** `{"symbol": "scip-python python myproject 1.2.0 app/greeting/greet()."}`

**result:**

```json
{
  "visibility": "public",
  "deprecated": { "since": "1.4.0", "replacement": "authenticate_v2", "message": "use authenticate_v2" }
}
```

`visibility` ∈ `public`, `protected`, `internal`, `private`, `unknown`. `deprecated` is OPTIONAL;
absent means not deprecated as far as the plugin can tell.

This verb backs a check that no general-purpose tool performs today: documentation that keeps
recommending an API the code has marked deprecated.

---

### 7.10 `parse_snippet` — Core

Does this code fragment parse? Cheap, high value, and answerable for hundreds of languages with a
parser alone.

**params:**

```json
{ "code": "def f(x):\n    return x + 1\n", "lang": "python", "dialect": "3.11", "mode": "module" }
```

`dialect` and `mode` are OPTIONAL. `mode` ∈ `module`, `expression`, `statements`, `fragment`;
default `module`.

**result:**

```json
{
  "ok": false,
  "diagnostics": [
    { "message": "unexpected token '}'", "range": { "start": {"line":2,"character":0}, "end": {"line":2,"character":1} }, "severity": "error" }
  ]
}
```

`evidence` MAY be `[]` for this verb (§5.4), since the code is supplied by the host and has no
repository location.

---

### 7.11 `references` — Optional

**params:** `{"symbol": "scip-python python myproject 1.2.0 app/greeting/greet().", "scope": "workspace", "limit": 200}`

`scope` ∈ `file`, `package`, `workspace`. `limit` is OPTIONAL.

**result:**

```json
{ "locations": [ ], "truncated": false }
```

Expensive. Hosts SHOULD NOT call this on every claim.

---

### 7.12 Verb summary

| Verb | Tier | Build-free (§3.3) | Primary use |
|---|---|---|---|
| `capabilities` | Required | MUST | Handshake |
| `resolve` | Core | MUST | Prose reference → SymbolId |
| `exists` | Core | MUST | Deleted/renamed API still documented |
| `location` | Core | — | Linking docs to code |
| `signature` | Core | — | Parameter/return/raises drift |
| `value` | Core | — | "the default is N" |
| `members` | Core | — | Enumeration drift, both directions |
| `docstring` | Core | — | Docstring vs prose |
| `visibility` | Optional | — | Documenting deprecated or private API |
| `parse_snippet` | Core | MUST | Doc code blocks that no longer parse |
| `references` | Optional | — | Impact analysis |

---

## 8. Capability negotiation

1. The host sends `capabilities` with the highest `cop` version it supports.
2. The plugin answers with `cop_versions`. The host selects the highest version both support and
   uses it for all subsequent requests.
3. The host MUST NOT send a verb absent from `result.verbs`.
4. If the host needs a verb the plugin does not implement, it MUST treat the corresponding claim
   as **unverified**, exactly as if `unsupported` had been returned. It MUST NOT synthesize a
   finding.
5. If the plugin declares `requires_build: true` for a verb and the host is running in no-build
   mode, the host MUST NOT send that verb.

*Rationale.* LSP's capability negotiation is what makes partial server implementations legitimate
rather than broken. COP copies it directly.

---

## 9. Errors

### 9.1 Per-request errors

A request that the plugin cannot process yields `status: "error"` with a one-line `reason`. The
plugin MUST continue serving subsequent requests.

```json
{ "cop": "0.1", "id": "r9", "status": "error",
  "reason": "malformed SymbolId: missing descriptor",
  "engine": { "name": "cop-python-ctags", "version": "0.1.0" } }
```

### 9.2 Unknown verbs

A plugin receiving a verb it does not implement MUST answer `status: "unsupported"`. It MUST NOT
crash and MUST NOT exit.

**Implementation note.** Decide whether the verb is supported *before* resolving the request's
subject. A dispatcher written in the obvious order —

```
symbol = lookup(params.symbol)          // not found -> answer not_found
switch (verb) { ...known verbs...; default: unsupported }
```

— returns `not_found` for every unimplemented verb, because the lookup fails first. That asserts
an absence the plugin never established, violating §3.4. Both reference plugins were written this
way initially; the conformance suite caught it in one of them and not the other, which is why
rule 13.3 probes an undeclared verb rather than trusting the dispatcher's shape.

### 9.3 Unparseable input

A line that is not valid JSON, or is valid JSON but lacks `id`, cannot be correlated. The plugin
SHOULD write a diagnostic to stderr and MAY exit with code 2. It MUST NOT emit an answer with a
fabricated `id`.

### 9.4 Host-side timeouts

If the host's deadline expires, the host MUST treat the outstanding request as `unsupported`, not
as `not_found` (§3.4). The host SHOULD terminate and restart the plugin process.

---

## 10. Determinism and caching

### 10.1 Determinism requirement

Given the same workspace content at the same revision, the same protocol version, and the same
request, a plugin MUST produce the **byte-identical** answer, `id` excluded.

This includes array ordering. Where this specification does not prescribe an order for an array,
the plugin is free to choose one, but MUST choose the same one every time — for example by sorting
on a stable key. *Unspecified order does not license nondeterministic order.* A plugin whose
`candidates` list comes out of a hash table in iteration order is non-conformant even though §7.2
only requires "descending plausibility".

Plugins MUST NOT let answers depend on wall-clock time, network state, or ambient environment
variables not declared in `cache_keys`.

*Rationale.* Nondeterministic gates get ignored. This is also what makes host-side caching sound.

### 10.2 Cache keys

The host SHOULD cache answers under a key combining:

- workspace revision (or content hash of the files in scope),
- contents of the files named by `cache_keys` (§7.1),
- `engine.name` and `engine.version`,
- the protocol version,
- the request `verb` and `params`.

`id` MUST NOT participate in the cache key.

---

## 11. Resource limits and sandboxing

The host is responsible for enforcement; plugin-declared `limits` are advisory.

The host MUST enforce a wall-clock timeout per plugin process and SHOULD enforce a memory ceiling.
This spec RECOMMENDS defaults of 60 s and 2 GiB for a batch, and 5 s for any single request in
interactive use.

The host SHOULD run plugins with:

- the workspace mounted read-only,
- a writable scratch directory,
- **no network access** by default.

A plugin that needs network access (to download a toolchain, for example) MUST declare it in its
manifest (§12) and the host MUST require explicit operator consent.

Plugins MUST NOT modify the workspace.

---

## 12. Plugin manifest and discovery

The wire protocol above says nothing about how a plugin is found, installed, or started. That is
the manifest's job.

A plugin repository provides `cop-plugin.yaml` at its root:

```yaml
cop_plugin: "0.1"
id: python-scip
description: Python code oracle backed by scip-python

runtime: python            # python | node | go | rust | ruby | docker | docker_image | system
entry: cop_python_scip:main
package: cop-python-scip
known_good_version: 0.3.1

languages: [python]
file_patterns: ["**/*.py", "**/*.pyi"]

network: false
requires_build: false

cache_keys: ["pyproject.toml", "setup.cfg", "requirements*.txt"]

# Capabilities: default is empty. What is not declared is not granted.
capabilities:
  read:    ["$WORKSPACE/**"]
  write:   ["$PLUGIN_TMP/**"]
  network: []                    # e.g. ["registry.npmjs.org:443"]
  exec:    ["ctags"]             # binaries this plugin may run
  env:     ["PATH", "HOME"]      # allowlist; never credentials
```

Rules:

- The manifest MUST live in the plugin's own repository. **There is no central registry.**
- The host is responsible for creating an isolated environment per `runtime`, installing the
  declared package at the pinned version, and invoking `entry`.
- `languages` and `file_patterns` here are for routing before the process starts; the authoritative
  values come from the `capabilities` answer.

- `COP-CAP-DECLARE` (PLUGIN) — A plugin MUST declare the capabilities it requires. Every capability
  defaults to empty.
- `COP-CAP-GRANT` (HOST) — The host MUST NOT grant a capability that was not declared. The host
  MUST enforce declared capabilities where the platform allows, and MUST report the enforcement
  level it achieved (§12.1).
- `COP-CAP-PIN` (HOST) — The host MUST record a digest of the declared capabilities in its lock
  file, so that a plugin update requesting new capabilities appears as a reviewable change.

### 12.1 Enforcement is uneven, and that must be visible

Enforcement of the capability declaration depends on the platform, and no portable mechanism
exists. The host MUST report which level it achieved:

| Platform | Mechanism | What is actually enforced |
|---|---|---|
| Linux ≥ 6.7 | Landlock ABI 4+ | filesystem paths, TCP bind/connect |
| Linux 5.13–6.6 | Landlock ABI 1–3 | filesystem paths only; **network not enforced** |
| macOS | `sandbox_init` (deprecated by Apple, no replacement) | best effort; network is all-or-nothing |
| Windows | — | **declaration only; nothing enforced** |
| all | clean environment, process-group kill, resource limits | always enforced |

`COP-CAP-DISCLOSE` (HOST) — The host MUST make the achieved enforcement level visible to the
operator and MUST record it in its audit log. Silently degrading to no enforcement turns this
section into the kind of unimplemented control this project exists to detect.

A specification that claims a sandbox it does not deliver is worse than one that claims nothing.
The honest statement is: this design defends against accident, bugs, and opportunistic misbehaviour
— not against an attacker who is targeting you.

*Rationale.* pre-commit's contract is a YAML file in the tool's own repository plus a `language:`
field telling the framework how to build an environment. There is no gatekeeper, so tool authors
own their own integration. That property, more than any technical detail, is why its ecosystem
grew. A curated central registry produces the opposite outcome: trunk-io/plugins is MIT-licensed
and openly contributable, and is nonetheless almost entirely vendor-authored.

---

## 13. Conformance

A **conformant plugin**:

| Rule | Requirement |
|---|---|
| `COP-CONF-HANDSHAKE` | implements `capabilities` and answers it before any other verb |
| `COP-CONF-PAIRING` | answers every request it reads with exactly one answer bearing the matching `id` |
| `COP-CONF-NOTFOUND` | never returns `not_found` when it means `unsupported` (§3.4) |
| `COP-CONF-CONFIDENCE` | never reports a `confidence` higher than the value declared in `capabilities` for that verb |
| `COP-CONF-EVIDENCE` | includes non-empty `evidence` for every `ok` answer except `capabilities` and `parse_snippet` |
| `COP-CONF-NOBUILD` | answers `capabilities`, `exists`, `resolve`, and `parse_snippet` without building the project |
| `COP-CONF-EXIT` | exits 0 when it has served its requests, regardless of the content of its answers |
| `COP-CONF-READONLY` | does not modify the workspace |
| `COP-CONF-DETERMINISM` | is deterministic per §10.1 |
| `COP-CONF-BATCH` | accepts a batched request array (§4.7) |
| `COP-CONF-NODEADLOCK` | does not deadlock the host when writing heavily to stderr (§4.2) |

All of the above are `PLUGIN` class.

A conformance fixture suite accompanies this specification (`conformance/`). Plugin authors run it
against their plugin and publish the resulting report.

A fixture is an input request plus an **assertion over the answer**, never a golden output — two
conformant plugins legitimately return different symbols, evidence, and confidence for the same
question, so exact-match fixtures would encode one implementation's choices as the standard.

Assertions are written as **JSON Schema subschemas applied to the whole answer**:

```json
{
  "name": "exists-absent-is-not_found",
  "spec": "3.4, 6.5",
  "request": { "verb": "exists", "params": { "ref": { "name": "ZzTotallyAbsentZz" } } },
  "expect": {
    "status": ["not_found"],
    "schema": {
      "type": "object",
      "required": ["searched"],
      "not": { "required": ["result"] }
    }
  }
}
```

Reusing JSON Schema here rather than inventing a predicate language is deliberate. Fixture authors
already know the vocabulary; the harness needs no second expression evaluator; and quantified
assertions — "every location in this answer, wherever it appears, has a workspace-relative path and
a zero-based range" — are expressible, which a flat path/operator predicate DSL cannot do.

### 13.1 Known-failure lists

`COP-CONF-FAILLIST` (PLUGIN) — An implementation SHOULD keep a committed list of the conformance
cases it does not yet pass, and its CI MUST run the suite against that list. The suite fails in
**both** directions:

- a case not on the list fails — a regression;
- a case **on** the list passes — the list is stale and must be updated.

This is the protobuf conformance suite's `failure_list` pattern. It records what an implementation
currently does without letting that record become the standard, which is why it is the only
golden-file pattern this specification endorses.

A list entry naming a case the suite does not have is reported but does not fail the run: a case
can be renamed or withdrawn between suite versions, and an implementation should not be broken by
a rule that no longer exists.

### 13.2 Unsupported cases

`COP-CONF-127` (PLUGIN) — A plugin driven by an external harness that does not implement a given
case MUST exit with status **127**, which the harness records as `UNSUPPORTED` rather than as a
failure.

Taken from the QUIC interop runner. It costs almost nothing and it means adding a new conformance
case does not turn every existing implementation's CI red.

*Rationale.* Kythe shipped a verifier that lets indexer authors self-certify against goals embedded
in test sources; Exercism ships a test-runner test suite. Self-certification is what lets an
ecosystem grow without the core team reviewing every plugin.

---

## 14. Versioning and deprecation

### 14.1 Version numbering

`cop` carries `MAJOR.MINOR`. The full spec version is `MAJOR.MINOR.PATCH`.

- **PATCH** — editorial only. No wire change.
- **MINOR** — additive only: new verbs, new optional fields, new enum values. A `0.N` plugin and a
  `0.N+1` host MUST interoperate.
- **MAJOR** — reserved for changes that cannot be made additively.

**Before 1.0.0, any release may break.** Do not build a third-party plugin against 0.x expecting
stability.

### 14.2 Additive-only rules

- **Verbs are never removed.** A verb that becomes obsolete is marked deprecated in the spec and
  continues to be listed; hosts stop calling it.
- **Fields are never removed or repurposed.** New optional fields are added; the must-ignore rule
  (§5.3) makes this safe.
- **Enum values are added, never removed.** Both parties MUST accept unrecognized enum values and
  degrade to the nearest documented default (`unknown` where one exists).

### 14.3 Deprecation policy

Once 1.0.0 ships:

- Anything deprecated remains functional for at least **two minor releases or one year**,
  whichever is longer.
- Breaking changes land only in a MAJOR release, and only in its first version.
- Anything marked `@experimental` in this document is exempt and may change in any release.

*Rationale.* ESLint's removal of the eslintrc system in v10 (February 2026) took the ecosystem
years to absorb; Bazel's WORKSPACE-to-bzlmod migration was similarly costly. SonarSource's plugin
API policy — minimum two years, breaking changes only in the first version of a major — is the
model copied here.

---

## 15. Normative rules index

Every normative rule in this document carries an identifier of the form `COP-<AREA>-<NAME>` and
names its class (§2.1). The index is **generated, not hand-written**: `cop-spec-check` extracts the
rules from this document, cross-references them against `covers:` annotations in the conformance
suite, and fails the build when a rule has no test.

```
$ cop-spec-check --coverage
COP-WIRE-COMPACT       PLUGIN   covered by 2 test(s)
COP-CONF-NODEADLOCK    PLUGIN   covered by 1 test(s)
COP-CAP-GRANT          HOST     UNCOVERED
```

A hand-maintained index of the rules would be a second copy of the same facts, kept in sync by
discipline. That is the failure mode this whole project exists to detect, so it is not one this
document is going to commit.

---

## 16. Emitting results

COP describes queries about code. What a host does with the answers is out of scope — except for
three findings that are easy to get wrong and that a host implementer should know before choosing
an output format.

**A finding here spans two locations** — a line of documentation and a line of code. Formats vary
in whether they can say that, and in whether any user interface shows it.

- **SARIF 2.1.0** carries a second location in `relatedLocations`, but GitHub code scanning uses
  only `locations[0]` and renders related locations *only where the result message embeds a link to
  them* using the `[text](id)` syntax of SARIF §3.11.6. A host that puts the second location in
  `relatedLocations` and stops there produces output in which the second location is invisible.
- GitHub's `partialFingerprints` handling reads **only** `primaryLocationLineHash`. A custom
  fingerprint keyed on a symbol identifier is ignored, so result identity in the host's own cache
  and result identity in GitHub's deduplication are different mechanisms and must be designed
  separately.
- GitHub ingests at most **5000 results** per run, discarding the rest in severity order without
  reporting that it did so, and rejects SARIF files above **10 MB** uncompressed. Any host that
  expects to emit a full-repository report needs a baseline and a new-findings-only mode from the
  start.

For the record: `SonarQube`'s generic issue import has first-class `secondaryLocations` and renders
them, and GitLab's Code Quality format has no second location at all. Emitting several formats is
ordinary practice — Ruff ships twelve.

---

## Appendix A — A minimal conformant plugin

The smallest useful plugin implements two verbs and needs no language knowledge beyond what
Universal Ctags already provides. A reference implementation accompanies this specification at
`crates/cop-ctags/`; it covers 135 languages and reports `confidence: "structural"` because ctags
has no scope or type resolution.

Two reference plugins ship with this specification, deliberately written in different languages
and runtimes:

| Plugin | Language | Backend | Confidence | Verbs |
|---|---|---|---|---|
| `cop-ctags` | Rust | Universal Ctags | `structural` | `exists`, `resolve`, `location` |
| `cop-typescript` | TypeScript | TypeScript type checker | `exact` | the above plus `signature`, `value`, `members`, `docstring`, `parse_snippet` |

A protocol with one implementation has not been shown to be language-neutral; it has only been
shown to be implementable once. The second implementation earned its keep immediately by failing
rule 13.3 in a way the first did not — see the implementation note in §9.2.

Its `capabilities` answer:

```json
{
  "cop_versions": ["0.1"],
  "engine": { "name": "cop-ctags", "version": "0.1.0" },
  "languages": ["*"],
  "verbs": {
    "exists":   { "confidence": "structural", "requires_build": false },
    "resolve":  { "confidence": "structural", "requires_build": false },
    "location": { "confidence": "structural", "requires_build": false }
  }
}
```

`languages: ["*"]` marks a **fallback plugin**: the host uses it only where no
language-specific plugin claims the file.

---

## Appendix B — Worked example: one claim, end to end

Documentation, `docs/config.md` line 42:

> The default request timeout is **30 seconds**.

Host extracts a `CONST` claim and asks:

```json
{"cop":"0.1","id":"c1","verb":"resolve","params":{"ref":{"name":"DEFAULT_TIMEOUT","lang":"python","context":{"path":"docs/config.md"}}}}
```

```json
{"cop":"0.1","id":"c1","status":"ok","confidence":"exact","result":{"symbol":"scip-python python myproject 1.2.0 app/config/DEFAULT_TIMEOUT.","kind":"constant"},"evidence":[{"path":"src/app/config.py","range":{"start":{"line":11,"character":0},"end":{"line":11,"character":15}},"is_definition":true}],"engine":{"name":"cop-python-scip","version":"0.3.1"}}
```

```json
{"cop":"0.1","id":"c2","verb":"value","params":{"symbol":"scip-python python myproject 1.2.0 app/config/DEFAULT_TIMEOUT."}}
```

```json
{"cop":"0.1","id":"c2","status":"ok","confidence":"exact","result":{"repr":"5.0","json":5.0,"type":"float","evaluation":"literal"},"evidence":[{"path":"src/app/config.py","range":{"start":{"line":11,"character":18},"end":{"line":11,"character":21}}}],"engine":{"name":"cop-python-scip","version":"0.3.1"}}
```

The host — not the plugin — now reasons: the prose says 30, the code says 5.0, confidence is
`exact`, the claim is specific and numeric, and `src/app/config.py` is in this pull request's diff.
It emits a blocking finding whose evidence points at both `docs/config.md:42` and
`src/app/config.py:11`.

Had the answer been `unsupported`, the host would have recorded the claim as unverified and
emitted nothing.

---

## Appendix C — Prior art this specification borrows from

| Source | What COP takes |
|---|---|
| **LSP** | Capability negotiation; the decision not to standardize semantics; zero-based UTF-16 positions |
| **SCIP** | The symbol string grammar (§6.2); human-readable IDs over opaque integers |
| **Exercism test runners** | Exit code describes the runner, not the result |
| **pre-commit** | Manifest in the tool's own repository; no central registry; host owns environment isolation |
| **Trunk** | Version-constrained command definitions; output parsers as separate processes |
| **reviewdog RDFormat** | Preserving original tool output alongside normalized data |
| **SARIF 2.1.0** | The result format, with the caveats in §16 |
| **SonarSource plugin API** | The deprecation policy in §14.3 |
| **Glean** | Language-specific richness first (`extra`), cross-language views derived on top |
| **Kythe** | What to avoid: build-system coupling, and a schema too large to implement |
| **stack-graphs** | What to avoid: a per-language authoring cost high enough to stall the ecosystem |

---

## Appendix D — Verification performed on this draft

| Check | Result |
|---|---|
| Every JSON example in this document validates against `schemas/cop-0.1.schema.json` | 22 examples, 0 failures |
| Every normative rule has a test, or a stated reason it does not | 36 rules; 32 tested, 3 pending the host implementation, 1 (`COP-ERR-FREE`, a permission) untestable |
| `cop-ctags` (Rust, ctags, `structural`) against the conformance suite | 32/32 |
| `cop-typescript` (TS type checker, `exact`) against the same suite | 32/32 |
| Host robustness against deliberately broken plugins | 12/12 survived |
| Metamorphic equivalence — key order, answer order, unknown-field injection | verdicts identical across 5 seeds |
| Mutation testing of the protocol library and reference plugin | 70/70 mutants caught |
| Property tests over framing, batching, symbol grammar, and UTF-16 ranges | 7 properties, 256 cases each |
| Dependency audit (`cargo deny`) | advisories, licenses, bans, sources — all clean |

Findings from that process are folded into the text above rather than left as test names.

The determinism rule (§10.1) originally exempted arrays whose order the spec does not prescribe,
which would have made a plugin that shuffles results conformant; it now requires byte-identical
answers. The dispatcher ordering trap (§9.2) was found by the second reference plugin, not by
review. The harness had no deadline of its own while §4.4 required one of hosts, and the first fix
did not work because killing a shell left its children holding the pipe — which is why
`COP-TIME-EXPIRE` names the process group rather than the process.

Mutation testing is what forced the last of these. A rule stated at `SHOULD` cannot fail a
conformance run, so the checks for §5.6 could not detect a plugin that stopped reporting its inputs
at all; the surviving mutants said so, and the gap is now closed by unit tests rather than by
raising a `SHOULD` this specification does not believe should be a `MUST`.
