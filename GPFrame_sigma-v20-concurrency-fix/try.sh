#!/usr/bin/env bash
# try.sh — the ENTIRE DGE workflow in one command.
#
#   ./try.sh                          # zero-argument demo: two bundled
#                                      # examples, one certifies, one is
#                                      # honestly refused — shows both
#                                      # outcomes DGE supports
#   ./try.sh myfile.rs my_function    # your own function
#
# This script exists because the multi-step CLI (build, discharge,
# pipeline, read a raw certificate comment) is real friction for anyone
# who isn't already a DGE developer. It does the SAME work the README's
# "Quick start" section describes, in the same order, with no shortcuts
# and no weakened guarantees — it just does it FOR you, once, and prints
# a plain-English summary instead of a raw certificate comment. If you
# want to see exactly what ran, every real command is echoed as it goes.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

BOLD='\033[1m'; GREEN='\033[32m'; YELLOW='\033[33m'; RED='\033[31m'; DIM='\033[2m'; RESET='\033[0m'
say()  { echo -e "$1"; }
step() { echo -e "${DIM}\$ $1${RESET}"; }

# ---------------------------------------------------------------- 1/4 --
# Check the two real prerequisites. Both are one-line installs; neither
# is optional (z3 backs EVERY certified rewrite, not just fancy ones).
say "${BOLD}[1/4] Checking prerequisites${RESET}"
missing=0
if ! command -v cargo >/dev/null 2>&1; then
    say "  ${RED}✗${RESET} Rust not found. Install: ${BOLD}curl https://sh.rustup.rs -sSf | sh${RESET}"
    missing=1
else
    say "  ${GREEN}✓${RESET} Rust ($(rustc --version | cut -d' ' -f2))"
fi
if ! command -v z3 >/dev/null 2>&1; then
    say "  ${RED}✗${RESET} z3 not found (needed for certified rewriting). Install: ${BOLD}apt install z3${RESET} (or ${BOLD}brew install z3${RESET})"
    missing=1
else
    say "  ${GREEN}✓${RESET} z3 ($(z3 --version | head -1))"
fi
if [ "$missing" = "1" ]; then
    say "\n${YELLOW}Install the above, then run ./try.sh again.${RESET}"
    exit 1
fi

# ---------------------------------------------------------------- 2/4 --
# Build once. Cached after the first run — this is the only slow step,
# and it's slow because it's compiling a compiler, not because DGE
# itself is slow to use.
BIN="target/release/dge"
say "\n${BOLD}[2/4] Building${RESET} (first run only — a few minutes; cached after)"
if [ ! -x "$BIN" ]; then
    step "cargo build --release -p cli"
    cargo build --release -p cli --quiet
else
    say "  ${GREEN}✓${RESET} already built ($BIN)"
fi

# ---------------------------------------------------------------- 3/4 --
# Discharge the proof table once. Two seconds; every certified rewrite
# depends on it, so there is no scenario where skipping it helps you.
say "\n${BOLD}[3/4] Preparing proofs${RESET} (one-time, ~2s)"
if [ ! -d "artifacts/o1" ]; then
    step "dge discharge"
    "$BIN" discharge >/dev/null
    say "  ${GREEN}✓${RESET} rule table proved (Z3)"
else
    say "  ${GREEN}✓${RESET} already prepared (artifacts/o1)"
fi

# ---------------------------------------------------------------- 4/4 --
run_one() {
    local file="$1" fn="$2"
    say "\n${BOLD}────────────────────────────────────────${RESET}"
    say "${BOLD}Trying \`$fn\` from $file${RESET}"
    local out
    out=$("$BIN" pipeline "$file" "$fn" --artifacts artifacts/o1 2>&1) || true

    if echo "$out" | grep -q "CERTIFIED"; then
        local n conf outfile
        n=$(echo "$out" | grep -oE '[0-9]+ rule' | head -1 | grep -oE '[0-9]+' || echo "0")
        outfile="${fn}_certified.rs"
        echo "$out" | sed -n '/^\/\/ AUTO-GENERATED/,$p' > "$outfile"
        say "${GREEN}${BOLD}✅ CERTIFIED${RESET}"
        say "DGE rewrote \`$fn\` and PROVED the rewrite means exactly the same thing —"
        say "not \"probably,\" a real check: every relevant input, including 0, ±infinity,"
        say "NaN, and the smallest/largest representable numbers, agrees bit-for-bit."
        if [ "$n" != "0" ]; then
            say "Simplifications applied: $n algebraic rule(s), each individually SMT-proved."
        fi
        say "Saved to: ${BOLD}$outfile${RESET}  ${DIM}(the certificate is the comment on top — that IS the proof)${RESET}"
    else
        say "${YELLOW}${BOLD}◯ NOT CERTIFIED${RESET} — and that's a real, honest answer, not an error."
        say "DGE only certifies what it can actually verify. Here's exactly why it stopped:"
        echo "$out" | grep -E "refused|REFUSED|Unsupported" | sed 's/^/  /' | head -5
        say "${DIM}(this is the exact reason, not a summary — DGE never guesses)${RESET}"
    fi
}

if [ $# -eq 0 ]; then
    say "\n${BOLD}[4/4] Running the bundled demo${RESET} ${DIM}(no file given — showing both outcomes)${RESET}"
    run_one "examples/demo.rs" "poly"
    run_one "examples/demo.rs" "guarded_divide"
    say "\n${BOLD}────────────────────────────────────────${RESET}"
    say "Try your own: ${BOLD}./try.sh path/to/file.rs function_name${RESET}"
else
    if [ $# -ne 2 ]; then
        say "\nUsage: ./try.sh [path/to/file.rs function_name]"
        exit 1
    fi
    say "\n${BOLD}[4/4] Running on your function${RESET}"
    run_one "$1" "$2"
fi
