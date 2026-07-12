# GPFrame Changelog

All notable changes to the GPFrame refactoring, testing, debugging, and JIT engine are documented in this file. GPFrame strictly adheres to semantic versioning and an uncompromising, certified-only release model.

## [v0.3.0] — 2026-07-12

### "Σ v1.2: Dynamic-Length Folds & Closed-Loop Pipeline"

This milestone completes the trusted-base extension for dynamic slices, introducing native loop codegen, automated back-emission, and the industry's first differential emission gate. The engine now compiles, refactors, and emits dynamic loops end-to-end with mathematical and statistical guarantees.

```
[Pure Rust &[f64]] ──extract──▶ fold() ──refactor──▶ VerifiedTerm ──emit──▶ Certified Rust (for loop)
       ▲                                                                         │
       └────────────────────────── Differential Emission Gate ───────────────────┘
                                   extract(emit(t)) ≡ t (10⁴ samples)
```

### Added

- **Dynamic Folds in** $\Sigma$**:** Introduced the `fold(init, body)` operator over $K$ parallel same-length sequences, bound to accumulator `acc` and element `elem k` binders.
    
    - _Hoist Semantics:_ Loop-invariant values are automatically hoisted by the optimizer.
        
    - _Safety:_ Binders are protected by an ownership validator preventing escape; `Term_p` remains total (iteration count is bound to runtime sequence length; $L=0 \implies \text{init}$).
        
- **Automatic Sequence Extraction:** The Extractor now accepts `&[f64]` slices as sequence parameters and automatically translates `for i in 0..s.len()` single-accumulator loops (including conditional updates) into native `fold` terms.
    
- **Sequence-Aware Test Harness:** - Extended the boundary-mixture sampler $\mu'$ with sequence length metrics (testing lengths $\{0, 1, 2\} \cup \text{uniform}[3, 32]$).
    
    - The counterexample shrinker now prioritizes minimizing sequence length first. Refutations carry minimized, highly readable witnesses of length $\le 2$.
        
- **9.3× JIT Loop Performance:** Cranelift JIT backend now features native loop codegen. The internal performance target ($\ge 5\times$ speedup) has achieved a measured **PASS (9.3× speedup on a 4096-length dot product)**.
    
- **Closed-Loop Code Generator (`dge emit`):** Translates optimized `Term_p` structures directly back to standard, readable Rust.
    
    - Emits Common Subexpression Elimination (CSE) `let` bindings in clean arena order.
        
    - Emits floating-point special values (`NaN`, $\pm\infty$) safely using `f64::from_bits` to prevent compiler-driven truncation or folding.
        
    - Translates internal mathematical `select` operators back into natural Rust conditional `if` statements.
        
    - Binds the cryptographic verification certificate directly to the code as a `/// doc comment` block.
        
- **Differential Emission Gate:** Verifies the code generator against the core parser by asserting $extract(emit(t)) \equiv t$ over $10^4$ random $\mu'$ samples, backed by a Rust-compiled runtime differential verification step.
    
- **Unified Command-Line Pipeline (`dge pipeline`):** Automates the entire loop-optimizing cycle in a single command: `extract` $\rightarrow$ `refactor` $\rightarrow$ `emit` $\rightarrow$ `gate`.
    
- **Rule-Trace Provenance:** Tier B certificates now carry a complete, verifiable audit trail of every algebraic rule matched and executed on the term.
    

### Changed

- The command-line `dge audit` utility now automatically reclassifies `0..s.len()` accumulator loops from `WITH_EFFORT` to `EXTRACT`.
    
- **Breaking ABI Change:** The JIT compilation layer's `RawFn` entrypoint now consumes raw sequence pointers and runtime length parameters, altering the downstream hot-dispatch interfaces.
    

### Limitations & Refusals (Roadmap Enforced)

- Dynamic loops are rejected by the SMT offline solver due to unboundedness; functions utilizing `fold` are limited to **Tier B certificates** (statistical equivalence over tested input spaces).
    
- The `memo` pure-function cache currently rejects terms containing `fold` due to sequence-aware key requirements.
    
- The extractor strictly rejects iterator adapters, windowed or offset indexing (`array[i-1]`), non-zero range starts, multi-accumulator states, and nested folds.
    

## [v0.2.0] — 2026-06-18

### "Imperative Kernels & Honest IEEE-754 Semantics"

This release dramatically expands the extraction perimeter to support modern imperative programming styles and replaces legacy mathematical encodings with bulletproof, first-class floating-point comparisons.

### Added

- **Extractor v2 (Imperative Kernel Support):**
    
    - Translates fixed-size array parameters (`[f64; N]` / `&[f64; N]`) directly into sequence var slots with compile-time index resolution.
        
    - Supports local variable mutation (`let mut` with `=`, `+=`, `-=`, `*=`, `/=`) by re-binding targets to Single Static Assignment (SSA) indices without compromising the mathematical totality of `Term_p`.
        
    - Supports literal-bounded `for i in LO..HI` and `..=` loops via compiler-driven unrolling (capped at 1024 iterations).
        
    - Handles statement-level `if` blocks with assignments in branches, resolving diverging bindings via mathematical `phi-merge` logic using `select`.
        
- **First-Class Comparisons in** $\Sigma$ **v1.1:** Adds `Lt`, `Gt`, `Le`, and `Ge` as native operations.
    
    - Evaluates to `1.0` (true) or `0.0` (false) with exact Rust/IEEE-754 semantics (including returning `false` on any `NaN` input and evaluating signed zeros $\pm0$ as equal).
        
    - Fully integrated across the interpreter, the `egg` e-graph rewrite rules, Cranelift JIT lowering (via ordered `fcmp` instructions), and the Z3 SMT solver (`fp.lt`).
        
- **Bitwise NaN Class Metric (`Metric::BitwiseNanClass`):** Created a unified metric to accommodate hardware and compiler differences in NaN generation. It enforces exact bitwise identity for all valid numbers, ensures correct $\pm0$ signs, but groups all arbitrary NaN payloads into a single equivalent class.
    

### Fixed & Discovered

- **Finding 7 (NaN Payloads are Non-Portable):** Discovered that LLVM's compile-time constant folder and local `x86_64` CPU hardware produce diverging bit-masks for certain operations (e.g., $-\infty + \infty$ yielding `0x7ff8...` vs `0xfff8...`). This proved that cross-generator bitwise comparison is physically impossible; GPFrame's validation gate now defaults to `BitwiseNanClass` to handle this accurately.
    
- **Finding 3b (EMA Filter FMA Cancellation):** The $\epsilon$-refutation gate successfully blocked and refuted FMA (Fused Multiply-Add) contraction on the Exponential Moving Average (EMA) filter. GPFrame proved that floating-point cancellation is scale-free; fusing math operations under catastrophic cancellation silently alters the output values regardless of domain boundaries.
    
- **Deleted Buggy Encodings:** Completely removed the old `min` and `sub`-based comparison encodings, which were exposed as unsound at the extraction boundary by the clamp kernel.
    

## [v0.1.0] — 2026-05-02

### "The Trusted Base"

- Initial release of the GPFrame engine.
    
- Implemented core mathematical language (`Term_p`) over 21 pure operators.
    
- Added the basic $\mu'$ boundary-mixture sampler and the initial Z3 rule-discharge engine.
    
- Implemented first-generation `egg` e-graph saturation and basic Cranelift JIT compilation pipelines.
