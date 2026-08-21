# Writing a COP plugin

A plugin reads JSON Lines on stdin and writes JSON Lines back. That is the whole contract. There is
no SDK you must use, no build system you must adopt, and no language you must write in — the shell
plugin in `plugins/hostile/` is forty-eight lines of bash and speaks the protocol correctly.

This guide takes you from nothing to a plugin the conformance suite calls conformant.

## 1. The smallest plugin that works

Answer `capabilities` and stop. A plugin that supports one verb is a valid plugin; a host will
simply not ask it anything else.

```bash
#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  printf '{"cop":"0.1","id":"%s","status":"ok","confidence":"textual","evidence":[],' "$id"
  printf '"result":{"cop_versions":["0.1"],"languages":["make"],"verbs":{},'
  printf '"limits":{"suggested_timeout_ms":5000,"max_batch":64}},'
  printf '"engine":{"name":"my-plugin","version":"0.1.0"}}\n'
done
```

Run the suite against it:

```bash
target/release/cop-conformance --plugin "sh ./my-plugin.sh" --workspace .
```

It will pass the handshake checks and skip everything else, because everything else is keyed on
what you declared in `verbs`. **Nothing you do not declare is ever asked of you.**

## 2. The four things that are easy to get wrong

These are not hypothetical. Each one shipped in a reference plugin in this repository and was
caught by the suite.

### `unsupported` is not `not_found`

`not_found` is a claim about the code: *I looked, and it is not there.* `unsupported` is a claim
about the plugin: *I cannot answer this kind of question.* A host turns the first into a finding
and the second into silence.

The bug is always in the dispatcher, and always the same shape:

```ts
const d = table.find(x => x.symbol === params.symbol);   // ← wrong: looks first
if (!d) return notFound(id, searched());
switch (verb) { /* ... */ default: return unsupported(id, verb); }
```

Every verb you did not implement now reports that the symbol does not exist. Check the verb first:

```ts
if (!SUPPORTED.includes(verb)) return unsupported(id, verb);
const d = table.find(x => x.symbol === params.symbol);
if (!d) return notFound(id, searched());
```

### Your `confidence` describes your mechanism, not your certainty

`exact` means a type-aware resolver answered. `structural` means a syntax tree answered and scopes
may be conflated. `textual` means a regex answered. It is not a probability, you do not tune it,
and a plugin that reports `exact` because it feels sure is lying in a field hosts use to decide
whether a mismatch is worth reporting.

### Determinism means byte-identical

Same input, same workspace, same answer bytes — every run. If you build a result from a hash map,
sort it before you emit it. The suite runs your plugin eight times and compares; an unsorted
two-element list passes a two-run check half the time, which is why it runs eight.

### Your exit code describes you, not your answers

Exit 0 when you handled the requests, whatever the answers were. Finding nothing is not failure;
it is an answer. Reserve non-zero for *you* being broken — and 127 specifically for "I do not
implement this suite", which the harness records as UNSUPPORTED rather than as a failure.

## 3. Writing to stdout will break you

Import one library that prints a deprecation warning to stdout and your protocol stream is corrupt.
The protocol channel exists to make that impossible:

```ts
const fd = Number(process.env.COP_PROTOCOL_FD);
const OUT = Number.isInteger(fd) && fd > 2 ? fd : 1;
```

When the host sets `COP_PROTOCOL_FD`, write answers there and stdout becomes a place noise can go
harmlessly. When it does not, use stdout and keep it clean. Support both: hosts are not required to
open the channel.

stderr is yours unconditionally. Log whatever you like; the host must never parse it and must never
read output on it as a sign of failure.

## 4. Report what you read

```json
"used_inputs": [
  { "kind": "tool",  "name": "ctags", "version": "Universal Ctags 6.1.0" },
  { "kind": "file",  "path": "src/greeting.py" }
]
```

The host caches your answers and needs to know when a diff invalidates one. Two rules pay for
themselves immediately:

- a positive answer about a file lists that file;
- a **negative** answer lists the *index*, not files. A miss depended on everything you searched,
  and a miss keyed on a file list can never be invalidated by the file whose creation would fix it.

The same applies to any answer that asserts nothing else matched — `resolve` returning a single
symbol is a claim about your whole index, so name the index there too.

Do not compute digests unless you already have them. You report *what* you read; the host owns the
cache and picks the hash.

## 5. Declare your capabilities

`cop-plugin.yaml` sits next to your plugin and says what it needs:

```yaml
cop_plugin: "0.2"
id: my-plugin
languages: ["python"]

capabilities:
  read:    ["$WORKSPACE/**"]
  write:   ["$PLUGIN_TMP/**"]
  network: []
  exec:    ["python3"]
  env:     ["PATH", "HOME"]

requires_build: false
network: false
```

Everything defaults to empty. What you do not declare is not granted, and a host that cannot
enforce a capability must say so rather than pretend. Keep this file honest: the suite's
`--manifest` mode fails you if what you declare and what you report from `capabilities` disagree.

## 6. Certify yourself

```bash
target/release/cop-conformance \
  --plugin "python3 my_plugin.py" \
  --workspace conformance/workspace \
  --fixtures conformance/fixtures \
  --schema  schemas/cop-0.1.schema.json \
  --manifest ./cop-plugin.yaml \
  --failure-list ./failure-list.txt
```

Warnings are SHOULD rules and never gate. Only MUST rules change the exit status.

Commit `failure-list.txt` even when it is empty. It records what you do not yet pass without
letting that record become the standard, and because a listed check that starts passing fails the
build as stale, the file cannot quietly outlive the gap it describes.

## 7. Then check that you survive being driven badly

The suite tests your plugin. `--hostile` tests the *host* — but if you are also writing a host,
run it, because it drives eleven deliberately broken plugins at yours and the failure modes are
the ones subprocess code always has:

```bash
target/release/cop-conformance --hostile \
  --plugin "sh plugins/hostile/hostile.sh" --workspace conformance/workspace
```

The one nobody tests is `stderr-flood`. A host that reads the protocol channel to completion before
draining stderr deadlocks the moment the plugin writes 64 KiB of logs, which on Linux is the pipe
buffer. It will not show up in development and it will show up in CI.

## Where to look next

- `SPEC.md` §7 — the eleven verbs and their exact params and results
- `crates/cop-ctags/src/main.rs` — a complete plugin in ~450 lines of Rust, `structural`
- `plugins/typescript/cop-typescript.ts` — a complete plugin over a real type checker, `exact`
- `conformance/fixtures/` — what the suite actually asserts, in a format you can extend
