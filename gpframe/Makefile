# deductive-gp-engine dev targets
.PHONY: test calib mutants discharge

test:
	cargo test --workspace

# R7 calibration — run in release for meaningful numbers
calib:
	cargo run --release -p cli --bin dge -- calib

# O1 discharge of the Dec rule table (requires z3 on PATH)
discharge:
	cargo run -p cli --bin dge -- discharge

# v1 M6: mutation-test THIS workspace's own suite with cargo-mutants.
# Long-running (rebuilds per mutant); intended for CI, not the inner loop.
mutants:
	cargo install cargo-mutants --locked 2>/dev/null || true
	cargo mutants --workspace --timeout 120 -- --release
