#!/usr/bin/env bun
/**
 * cop-typescript — a Code Oracle Protocol plugin for TypeScript and JavaScript.
 *
 * Backed by the TypeScript compiler's own type checker, so it answers at
 * `exact` confidence (SPEC 6.4): name resolution and types are as reliable as
 * the language's own tooling.
 *
 * This plugin exists for two reasons. First, it is genuinely useful — the
 * signature and value verbs need a type checker to be trustworthy. Second, it
 * is written in a different language and runtime from the reference plugin and
 * the conformance runner, which is the only way to find out whether the
 * protocol is actually language-neutral or merely claims to be.
 */
import * as ts from "typescript";
import * as fs from "node:fs";
import * as path from "node:path";

const COP_VERSION = "0.1";
const ENGINE = { name: "cop-typescript", version: "0.1.0" };
const SCHEME = "scip-typescript";

type Confidence = "exact" | "structural" | "textual";
type Status = "ok" | "not_found" | "ambiguous" | "unsupported" | "error";

interface Position { line: number; character: number }
interface Range { start: Position; end: Position }
interface Location { path: string; range: Range; is_definition?: boolean }

interface UsedInput {
  kind: string; path?: string; name?: string; version?: string; scope?: string;
}

interface Answer {
  cop: string; id: string; status: Status;
  confidence?: Confidence;
  result?: unknown;
  evidence?: Location[];
  searched?: unknown;
  candidates?: unknown[];
  used_inputs?: UsedInput[];
  engine: typeof ENGINE;
  reason?: string;
}

const ok = (id: string, result: unknown, evidence: Location[], confidence: Confidence = "exact"): Answer =>
  ({ cop: COP_VERSION, id, status: "ok", confidence, result, evidence, engine: ENGINE });

const notFound = (id: string, searched: unknown): Answer =>
  ({ cop: COP_VERSION, id, status: "not_found", confidence: "exact", searched, engine: ENGINE });

const ambiguous = (id: string, candidates: unknown[]): Answer =>
  ({ cop: COP_VERSION, id, status: "ambiguous", confidence: "exact", candidates, engine: ENGINE });

/** SPEC 3.4: carries no information about the code. Never a finding. */
const unsupported = (id: string, reason: string): Answer =>
  ({ cop: COP_VERSION, id, status: "unsupported", reason, engine: ENGINE });

const errored = (id: string, reason: string): Answer =>
  ({ cop: COP_VERSION, id, status: "error", reason, engine: ENGINE });

// ── used inputs (SPEC 5.6) ──────────────────────────────────────────────────

/**
 * A type checker is a whole-program analyser: a signature can change because a
 * file the answer never names changed an imported type. So every answer about
 * the repository reports the program alongside the files behind its evidence,
 * and no answer claims a narrower dependency than it has.
 *
 * No digests. The plugin reports what it read; the host owns the cache and
 * decides how to key it.
 */
function inputsFor(a: Answer): UsedInput[] {
  const inputs: UsedInput[] = [{ kind: "tool", name: "typescript", version: ts.version }];
  const seen = new Set<string>();
  const locs: Location[] = [
    ...(a.evidence ?? []),
    ...((a.candidates ?? []) as { evidence?: Location[] }[]).flatMap((c) => c.evidence ?? []),
  ];
  for (const l of locs) {
    if (!seen.has(l.path)) { seen.add(l.path); inputs.push({ kind: "file", path: l.path }); }
  }
  inputs.push({ kind: "index", name: "ts-program", scope: "workspace" });
  return inputs;
}

/** Attach what the answer read. Answers that read nothing say nothing. */
function withInputs(a: Answer, verb: string): Answer {
  // `capabilities` is a statement about the plugin, not about the code.
  if (verb === "capabilities") return a;
  if (a.status === "unsupported" || a.status === "error") return a;
  // `parse_snippet` reads the snippet the host supplied and no repository file.
  if (verb === "parse_snippet") {
    return { ...a, used_inputs: [{ kind: "tool", name: "typescript", version: ts.version }] };
  }
  return { ...a, used_inputs: inputsFor(a) };
}

// ── program construction ────────────────────────────────────────────────────

const WORKSPACE = process.env.COP_WORKSPACE ?? ".";

function sourceFiles(root: string): string[] {
  // Absolute paths throughout: ts keeps `fileName` exactly as given, so mixing
  // relative input with absolute comparisons silently matches nothing.
  const base = path.resolve(root);
  const out: string[] = [];
  const walk = (dir: string) => {
    let entries: fs.Dirent[];
    try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return; }
    for (const e of entries.sort((a, b) => a.name.localeCompare(b.name))) {
      if (e.name === "node_modules" || e.name.startsWith(".")) continue;
      const p = path.join(dir, e.name);
      if (e.isDirectory()) walk(p);
      else if (/\.(ts|tsx|mts|cts|js|jsx|mjs|cjs)$/.test(e.name)) out.push(p);
    }
  };
  walk(base);
  return out;
}

let _program: ts.Program | null = null;
let _files: string[] = [];

function program(): ts.Program {
  if (_program) return _program;
  _files = sourceFiles(WORKSPACE);
  _program = ts.createProgram(_files, {
    allowJs: true, checkJs: false, noEmit: true,
    target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext,
    skipLibCheck: true, noResolve: false,
  });
  // Parent pointers are set during binding, and binding is triggered by creating
  // the checker. Walking the AST before this leaves `node.parent` undefined.
  _program.getTypeChecker();
  return _program;
}

const searched = () => ({
  scope: "workspace", roots: [path.relative(process.cwd(), WORKSPACE) || "."],
  file_count: _files.length, note: "TypeScript program; type-checked",
});

// ── symbol handling (SPEC 6.2) ──────────────────────────────────────────────

const TYPE_KINDS = new Set(["class", "interface", "enum", "type_alias"]);
const CALLABLE_KINDS = new Set(["function", "method", "constructor"]);

function descriptor(name: string, kind: string): string {
  if (TYPE_KINDS.has(kind)) return `${name}#`;
  if (CALLABLE_KINDS.has(kind)) return `${name}().`;
  return `${name}.`;
}

function relPath(f: ts.SourceFile): string {
  return path.relative(path.resolve(WORKSPACE), path.resolve(f.fileName)).split(path.sep).join("/");
}

function makeSymbol(f: ts.SourceFile, scope: string | undefined, name: string, kind: string): string {
  const segs = relPath(f).split("/").filter(Boolean).map((s) => `${s}/`).join("");
  return `${SCHEME} . . . ${segs}${scope ? `${scope}#` : ""}${descriptor(name, kind)}`;
}

function kindOf(n: ts.Node): string {
  if (ts.isClassDeclaration(n)) return "class";
  if (ts.isInterfaceDeclaration(n)) return "interface";
  if (ts.isEnumDeclaration(n)) return "enum";
  if (ts.isEnumMember(n)) return "enum_member";
  if (ts.isTypeAliasDeclaration(n)) return "type_alias";
  if (ts.isFunctionDeclaration(n)) return "function";
  if (ts.isMethodDeclaration(n) || ts.isMethodSignature(n)) return "method";
  if (ts.isConstructorDeclaration(n)) return "constructor";
  if (ts.isPropertyDeclaration(n) || ts.isPropertySignature(n)) return "property";
  if (ts.isVariableDeclaration(n)) {
    const flags = (n.parent as ts.VariableDeclarationList | undefined)?.flags ?? 0;
    return flags & ts.NodeFlags.Const ? "constant" : "variable";
  }
  return "unknown";
}

interface Decl { node: ts.Node; file: ts.SourceFile; name: string; scope?: string; kind: string; symbol: string }

function allDeclarations(): Decl[] {
  const out: Decl[] = [];
  for (const f of program().getSourceFiles()) {
    if (f.isDeclarationFile || !f.fileName.startsWith(path.resolve(WORKSPACE))) continue;
    const visit = (n: ts.Node, scope?: string) => {
      const nameNode = (n as { name?: ts.Node }).name;
      if (nameNode && ts.isIdentifier(nameNode)) {
        const kind = kindOf(n);
        if (kind !== "unknown") {
          out.push({ node: n, file: f, name: nameNode.text, scope, kind,
                     symbol: makeSymbol(f, scope, nameNode.text, kind) });
        }
        const nextScope = TYPE_KINDS.has(kind) ? nameNode.text : scope;
        ts.forEachChild(n, (c) => visit(c, nextScope));
        return;
      }
      ts.forEachChild(n, (c) => visit(c, scope));
    };
    ts.forEachChild(f, (c) => visit(c, undefined));
  }
  // SPEC 10.1: unspecified order does not license nondeterministic order.
  out.sort((a, b) => (relPath(a.file) + a.symbol).localeCompare(relPath(b.file) + b.symbol));
  return out;
}

let _decls: Decl[] | null = null;
const decls = () => (_decls ??= allDeclarations());

function locationOf(d: Decl): Location {
  const nameNode = (d.node as { name?: ts.Node }).name!;
  const s = d.file.getLineAndCharacterOfPosition(nameNode.getStart(d.file));
  const e = d.file.getLineAndCharacterOfPosition(nameNode.getEnd());
  return { path: relPath(d.file), range: { start: s, end: e }, is_definition: true };
}

function splitRef(name: string): [string | undefined, string] {
  for (const sep of ["::", ".", "#"]) {
    const i = name.lastIndexOf(sep);
    if (i > 0) return [name.slice(0, i).split(sep).pop() || undefined, name.slice(i + sep.length)];
  }
  return [undefined, name];
}

function lookup(ref: { name: string; kind_hint?: string }): Decl[] {
  const [scope, name] = splitRef(ref.name);
  let hits = decls().filter((d) => d.name === name);
  if (scope) {
    const scoped = hits.filter((d) => d.scope === scope);
    if (scoped.length) hits = scoped;
  }
  if (ref.kind_hint && ref.kind_hint !== "unknown") {
    const typed = hits.filter((d) => d.kind === ref.kind_hint);
    if (typed.length) hits = typed;
  }
  return hits;
}

// ── verbs ───────────────────────────────────────────────────────────────────

function capabilities(id: string): Answer {
  const v = (requires_build = false) => ({ confidence: "exact" as const, requires_build });
  return ok(id, {
    cop_versions: [COP_VERSION],
    engine: ENGINE,
    languages: ["typescript", "javascript", "typescriptreact", "javascriptreact"],
    file_patterns: ["**/*.ts", "**/*.tsx", "**/*.mts", "**/*.cts", "**/*.js", "**/*.jsx"],
    verbs: {
      exists: v(), resolve: v(), location: v(),
      signature: v(), value: v(), members: v(), docstring: v(),
      parse_snippet: v(),
    },
    cache_keys: ["tsconfig.json", "package.json"],
    limits: { suggested_timeout_ms: 60000, max_batch: 512 },
  }, [], "exact");
}

function signatureOf(id: string, d: Decl): Answer {
  const checker = program().getTypeChecker();
  const sym = checker.getSymbolAtLocation((d.node as { name?: ts.Node }).name!);
  if (!sym) return unsupported(id, "no symbol at declaration");
  const type = checker.getTypeOfSymbolAtLocation(sym, d.node);
  const sigs = type.getCallSignatures();
  if (!sigs.length) return unsupported(id, `'${d.name}' is not callable`);
  const sig = sigs[0]!;

  const params = sig.getParameters().map((p) => {
    const decl = p.valueDeclaration as ts.ParameterDeclaration | undefined;
    return {
      name: p.getName(),
      kind: "positional",
      type: checker.typeToString(checker.getTypeOfSymbolAtLocation(p, d.node)),
      ...(decl?.initializer ? { default: decl.initializer.getText(d.file) } : {}),
      optional: !!decl?.questionToken || !!decl?.initializer,
      variadic: !!decl?.dotDotDotToken,
    };
  });

  const mods = (ts.canHaveModifiers(d.node) ? ts.getModifiers(d.node) ?? [] : [])
    .map((m) => ts.tokenToString(m.kind)!)
    .filter(Boolean);

  return ok(id, {
    render: checker.signatureToString(sig),
    params,
    returns: { type: checker.typeToString(sig.getReturnType()) },
    raises: [],
    type_params: (sig.getTypeParameters() ?? []).map((t) => ({ name: t.symbol.getName() })),
    modifiers: mods,
  }, [locationOf(d)]);
}

function valueOf(id: string, d: Decl): Answer {
  const init = (d.node as { initializer?: ts.Expression }).initializer;
  if (!init) return unsupported(id, `'${d.name}' has no initializer`);
  const checker = program().getTypeChecker();
  const repr = init.getText(d.file);
  let json: unknown;
  let evaluation = "unknown";
  if (ts.isNumericLiteral(init)) { json = Number(init.text); evaluation = "literal"; }
  else if (ts.isStringLiteral(init)) { json = init.text; evaluation = "literal"; }
  else if (init.kind === ts.SyntaxKind.TrueKeyword) { json = true; evaluation = "literal"; }
  else if (init.kind === ts.SyntaxKind.FalseKeyword) { json = false; evaluation = "literal"; }
  else if (ts.isPrefixUnaryExpression(init) && ts.isNumericLiteral(init.operand)) {
    json = init.operator === ts.SyntaxKind.MinusToken ? -Number(init.operand.text) : Number(init.operand.text);
    evaluation = "const_folded";
  }
  const type = checker.typeToString(checker.getTypeAtLocation(init));
  const s = d.file.getLineAndCharacterOfPosition(init.getStart(d.file));
  const e = d.file.getLineAndCharacterOfPosition(init.getEnd());
  return ok(id,
    { repr, ...(json !== undefined ? { json } : {}), type, evaluation },
    [{ path: relPath(d.file), range: { start: s, end: e } }]);
}

function membersOf(id: string, d: Decl, kinds?: string[]): Answer {
  const members = decls()
    .filter((m) => m.scope === d.name && relPath(m.file) === relPath(d.file))
    .filter((m) => !kinds?.length || kinds.includes(m.kind))
    .map((m) => ({ symbol: m.symbol, name: m.name, kind: m.kind }));
  return ok(id, { members, complete: true }, [locationOf(d)]);
}

function docstringOf(id: string, d: Decl): Answer {
  const checker = program().getTypeChecker();
  const sym = checker.getSymbolAtLocation((d.node as { name?: ts.Node }).name!);
  const text = sym ? ts.displayPartsToString(sym.getDocumentationComment(checker)) : "";
  if (!text) return notFound(id, searched());
  return ok(id, { text, format: "markdown" }, [locationOf(d)]);
}

function parseSnippet(id: string, code: string, lang?: string): Answer {
  const jsx = lang?.includes("react") || lang === "tsx" || lang === "jsx";
  const sf = ts.createSourceFile(
    `snippet.${jsx ? "tsx" : "ts"}`, code, ts.ScriptTarget.ESNext, true,
    jsx ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  // `parseDiagnostics` is internal but is the only syntactic-error channel for a
  // standalone SourceFile; a program-based check would require a real file.
  const diags = ((sf as unknown as { parseDiagnostics?: ts.DiagnosticWithLocation[] }).parseDiagnostics) ?? [];
  const diagnostics = diags.map((dg) => {
    const s = sf.getLineAndCharacterOfPosition(dg.start);
    const e = sf.getLineAndCharacterOfPosition(dg.start + dg.length);
    return {
      message: ts.flattenDiagnosticMessageText(dg.messageText, " "),
      range: { start: s, end: e },
      severity: "error" as const,
    };
  });
  // SPEC 5.4: parse_snippet is evidence-exempt — the code has no repo location.
  return ok(id, { ok: diagnostics.length === 0, diagnostics }, []);
}

// ── dispatch ────────────────────────────────────────────────────────────────

function handle(req: { id: string; verb: string; params: Record<string, unknown> }): Answer {
  return withInputs(answerFor(req), req.verb);
}

function answerFor(req: { id: string; verb: string; params: Record<string, unknown> }): Answer {
  const { id, verb, params } = req;
  if (verb === "capabilities") return capabilities(id);

  if (verb === "parse_snippet") {
    const code = params.code;
    if (typeof code !== "string") return errored(id, "params.code must be a string");
    return parseSnippet(id, code, params.lang as string | undefined);
  }

  // Verbs keyed by SymbolId resolve through the declaration table.
  const bySymbol = (sym: string) => decls().find((d) => d.symbol === sym);

  if (verb === "exists" || verb === "resolve") {
    if (typeof params.symbol === "string" && !params.ref) {
      const d = bySymbol(params.symbol);
      return d ? ok(id, { exists: true, kind: d.kind }, [locationOf(d)]) : notFound(id, searched());
    }
    const ref = params.ref as { name: string; kind_hint?: string } | undefined;
    if (!ref?.name) return errored(id, "missing 'ref' or 'symbol' in params");
    const hits = lookup(ref);
    if (!hits.length) return notFound(id, searched());
    if (verb === "exists") {
      return ok(id, { exists: true, kind: hits[0]!.kind }, hits.slice(0, 8).map(locationOf));
    }
    if (hits.length > 1) {
      return ambiguous(id, hits.slice(0, 8).map((d) => ({
        symbol: d.symbol, kind: d.kind, evidence: [locationOf(d)],
      })));
    }
    return ok(id, { symbol: hits[0]!.symbol, kind: hits[0]!.kind }, [locationOf(hits[0]!)]);
  }

  // SPEC 3.4 and 9.2: decide whether the verb is supported BEFORE looking the
  // symbol up. Resolving first turns every unimplemented verb into `not_found`,
  // which asserts absence the plugin never established. This is the easiest
  // mistake to make in a dispatcher and the most damaging one.
  const SYMBOL_VERBS = ["location", "signature", "value", "members", "docstring"];
  if (!SYMBOL_VERBS.includes(verb)) {
    return unsupported(id, `verb '${verb}' not implemented by cop-typescript`);
  }

  const sym = params.symbol;
  if (typeof sym !== "string") return errored(id, "params.symbol must be a string");
  const d = bySymbol(sym);
  if (!d) return notFound(id, searched());

  switch (verb) {
    case "location":  return ok(id, { locations: [locationOf(d)] }, [locationOf(d)]);
    case "signature": return signatureOf(id, d);
    case "value":     return valueOf(id, d);
    case "members":   return membersOf(id, d, params.kinds as string[] | undefined);
    case "docstring": return docstringOf(id, d);
    default:          return unsupported(id, `verb '${verb}' not implemented by cop-typescript`);
  }
}

// ── protocol channel + batching (SPEC 4.6, 4.7) ─────────────────────────────

/** covers: COP-FD-USE COP-WIRE-CHANNEL — fd 3 when the host opened it. */
const PROTOCOL_FD = (() => {
  const v = Number(process.env.COP_PROTOCOL_FD);
  return Number.isInteger(v) && v > 2 ? v : null;
})();

/** covers: COP-WIRE-COMPACT — exactly one line per answer. */
function send(answer: Answer): void {
  const line = JSON.stringify(answer) + "\n";
  if (PROTOCOL_FD !== null) {
    try { fs.writeSync(PROTOCOL_FD, line); return; } catch { /* fall through */ }
  }
  process.stdout.write(line);
}

/** covers: COP-BATCH-ACCEPT COP-BATCH-EMPTY */
function parseLine(text: string): { id?: string; verb?: string; params?: Record<string, unknown> }[] {
  const v = JSON.parse(text);
  return Array.isArray(v) ? v : [v];
}

// ── stdio loop (SPEC 4) ─────────────────────────────────────────────────────

async function main(): Promise<number> {
  const input = await new Response(Bun.stdin.stream()).text();
  for (const line of input.split("\n")) {
    const t = line.trim();
    if (!t) continue;
    let reqs: { id?: string; verb?: string; params?: Record<string, unknown> }[];
    try {
      reqs = parseLine(t);
    } catch (e) {
      console.error(`cop-typescript: unparseable input: ${e}`); // SPEC 9.3
      return 2;
    }
    for (const req of reqs) {
      if (!req.id) {
        console.error("cop-typescript: request without id");
        return 2;
      }
      let answer: Answer;
      try {
        answer = handle({ id: req.id, verb: req.verb ?? "", params: req.params ?? {} });
      } catch (e) {
        answer = errored(req.id, `internal: ${e instanceof Error ? e.message : String(e)}`);
      }
      send(answer);
    }
  }
  return 0; // SPEC 4.5: exit code describes the plugin, never the answers.
}

process.exit(await main());
