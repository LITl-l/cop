.PHONY: check build test spec-examples spec-coverage conformance hostile mischief audit mutants semver clean
BIN := target/release

check: build test spec-examples spec-coverage conformance hostile mischief audit

build:
	cargo build --release

test:
	cargo test --workspace --quiet

spec-examples: build
	$(BIN)/cop-spec-check SPEC.md schemas/cop-0.1.schema.json

# Every MUST rule in SPEC.md must have at least one test that names it.
# Gating: every MUST rule this suite can test must have a test that names it.
# Rules constraining a host feature not implemented here report PENDING-HOST.
spec-coverage: build
	@$(BIN)/cop-spec-check --coverage SPEC.md | tail -4

# Both reference plugins must pass the same suite. Two implementations in
# different languages and runtimes agreeing is the only evidence the protocol is
# language-neutral rather than merely claiming to be.
conformance: build
	@echo "== cop-ctags (Rust, 135 languages, structural) =="
	@$(BIN)/cop-conformance --plugin "$(BIN)/cop-ctags" \
	  --workspace conformance/workspace --fixtures conformance/fixtures \
	  --schema schemas/cop-0.1.schema.json --manifest crates/cop-ctags/cop-plugin.yaml \
	  --failure-list conformance/failure-list/cop-ctags.txt
	@echo
	@echo "== cop-typescript (TypeScript/Bun, exact) =="
	@$(BIN)/cop-conformance --plugin "bun plugins/typescript/cop-typescript.ts" \
	  --workspace conformance/workspace --fixtures conformance/fixtures \
	  --schema schemas/cop-0.1.schema.json --manifest plugins/typescript/cop-plugin.yaml \
	  --failure-list conformance/failure-list/cop-typescript.txt

# Drive deliberately broken plugins and check the HOST survives. This suite
# tests the host, not the plugin.
hostile: build
	@$(BIN)/cop-conformance --hostile --plugin "./plugins/hostile/hostile.sh" \
	  --workspace conformance/workspace

# Advisories, licences, duplicate versions and source provenance in one gate.
audit:
	cargo deny check

clean:
	cargo clean

# Metamorphic: the specification permits key order, answer order, and unknown
# fields to vary. Run the same suite through a wrapper that varies all three,
# deterministically per seed, and require the verdicts to be identical.
# A difference means the host depends on something the specification does not
# promise. (Let's Encrypt ships Pebble for exactly this reason.)
SEEDS ?= 1 7 42 1337 90210
mischief: build
	@rm -f /tmp/cop-mis-*.txt; fail=0; \
	for s in $(SEEDS); do \
	  $(BIN)/cop-conformance \
	    --plugin "$(BIN)/cop-mischief --seed $$s -- $(BIN)/cop-ctags" \
	    --workspace conformance/workspace --fixtures conformance/fixtures \
	    --schema schemas/cop-0.1.schema.json --manifest crates/cop-ctags/cop-plugin.yaml 2>/dev/null \
	    | grep -E '^  \[' | awk '{print $$1, $$2}' > /tmp/cop-mis-$$s.txt; \
	  n=$$(grep -c PASS /tmp/cop-mis-$$s.txt); f=$$(grep -c FAIL /tmp/cop-mis-$$s.txt || true); \
	  echo "  seed $$s: $$n pass, $$f fail"; \
	done; \
	first=""; \
	for s in $(SEEDS); do \
	  if [ -z "$$first" ]; then first=/tmp/cop-mis-$$s.txt; \
	  elif ! diff -q $$first /tmp/cop-mis-$$s.txt >/dev/null; then \
	    echo "  seed $$s produced different verdicts:"; diff $$first /tmp/cop-mis-$$s.txt | head -10; fail=1; \
	  fi; \
	done; \
	if [ $$fail -eq 0 ]; then echo "  verdicts identical across all seeds"; else exit 1; fi

# The published API of cop-core is what third-party plugins compile against, so
# a breaking change must be a deliberate version decision rather than a diff
# nobody noticed. BASELINE is a git ref, which works before the crate is
# published; after publication, drop --baseline-rev and it compares to crates.io.
# Not part of `make check`: it clones and builds the baseline.
BASELINE ?= v0.3.0-rc1
semver:
	@if ! command -v cargo-semver-checks >/dev/null 2>&1; then \
	  echo "  cargo-semver-checks not installed: cargo install cargo-semver-checks --locked"; exit 1; fi
	@if ! git rev-parse --verify -q $(BASELINE) >/dev/null; then \
	  echo "  no baseline ref $(BASELINE); set BASELINE=<ref>"; exit 1; fi
	cargo semver-checks --baseline-rev $(BASELINE) -p cop-core

# Mutation testing: break the reference plugin on purpose and ask whether the
# conformance suite notices. A surviving mutant is a hole in the suite.
# Slow (~50s); not part of `make check`.
mutants: build
	COP_SUITE_ROOT="$$PWD" cargo mutants --package cop-ctags --package cop-core -j 2 --timeout 120
