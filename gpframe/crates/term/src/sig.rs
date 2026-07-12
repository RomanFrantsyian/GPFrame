//! [A1] Σ — the signature. ~20 pure ops over f64, arity table, per-op FP notes.
//!
//! O5 by construction: nothing here can perform IO, allocate shared state, or
//! mutate an environment. Adding an effectful symbol is an architecture bug.
//!
//! FP semantics contract (feeds harness::metric and rules::r_approx):
//! * All ops are IEEE-754 binary64, round-to-nearest-even.
//! * `Fma` is a *distinct symbol* (single rounding) — it is NOT `Add(Mul(..))`;
//!   the two are related only by an R_approx rule under `~_eps`.
//! * Transcendentals (`Sin..Ln`) have no decidable SMT theory (T2) — any rule
//!   mentioning them routes to Tier B always (v2.1 §2), and their runtime
//!   values are pinned to the libm build via the O8 env fingerprint.

/// Operator tags of Σ. Keep this in one screen — it is trusted base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Op {
    // -- nullary --------------------------------------------------------
    /// Constant; payload = index into `Term::consts`.
    Const,
    /// Free variable; payload = index into the environment `Env`.
    Var,
    // -- unary ----------------------------------------------------------
    Neg,
    Abs,
    Sqrt,
    Floor,
    Ceil,
    // transcendental (Tier B only, see module doc)
    Sin,
    Cos,
    Tan,
    Exp,
    Ln,
    // -- binary ---------------------------------------------------------
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Pow,
    // ordered comparisons, 1.0/0.0-valued; IEEE/Rust semantics: FALSE when
    // either operand is NaN; ±0 compare equal. First-class (Σ v1.1) so the
    // extractor needs no NaN-caveated encodings and SMT gets fp.lt/gt/leq/geq.
    Lt,
    Gt,
    Le,
    Ge,
    // -- sequence fold (Σ v1.2) ------------------------------------------
    /// fold(init, body) over K parallel same-length runtime sequences.
    /// Body may use Acc and Elem(k); iteration count = the sequences' L.
    /// L = 0 ⇒ result = init. Unbounded data ⇒ no decidable SMT theory
    /// (T2): rules never rewrite under fold; Tier B gates only.
    Fold,
    /// current accumulator — valid ONLY inside a fold body (validated).
    Acc,
    /// current element of sequence `k` (payload) — body-only (validated).
    Elem,
    // -- ternary --------------------------------------------------------
    /// Fused multiply-add: a*b + c with a single rounding.
    Fma,
    /// select(cond, then, else): cond != 0.0 → then, else → else.
    /// The only branching symbol; keeps Term_p total (no partial match).
    Select,
}

impl Op {
    /// Arity table. Const/Var carry payloads, not children.
    pub const fn arity(self) -> usize {
        use Op::*;
        match self {
            Const | Var | Acc | Elem => 0,
            Neg | Abs | Sqrt | Floor | Ceil | Sin | Cos | Tan | Exp | Ln => 1,
            Add | Sub | Mul | Div | Min | Max | Pow | Lt | Gt | Le | Ge | Fold => 2,
            Fma | Select => 3,
        }
    }

    /// Tier routing hint (v2.1 §2): transcendental-bearing ⇒ Tier B always.
    pub const fn is_transcendental(self) -> bool {
        matches!(self, Op::Sin | Op::Cos | Op::Tan | Op::Exp | Op::Ln | Op::Pow)
    }

    /// Inverse of `name()` for non-payload ops (parser use).
    pub fn from_name(s: &str) -> Option<Op> {
        use Op::*;
        const ALL: &[Op] = &[
            Neg, Abs, Sqrt, Floor, Ceil, Sin, Cos, Tan, Exp, Ln,
            Add, Sub, Mul, Div, Min, Max, Pow, Lt, Gt, Le, Ge, Fma, Select, Fold,
        ];
        ALL.iter().copied().find(|op| op.name() == s)
    }

    /// Stable name for s-expressions and hashing salt.
    pub const fn name(self) -> &'static str {
        use Op::*;
        match self {
            Const => "const", Var => "var",
            Neg => "neg", Abs => "abs", Sqrt => "sqrt",
            Floor => "floor", Ceil => "ceil",
            Sin => "sin", Cos => "cos", Tan => "tan", Exp => "exp", Ln => "ln",
            Add => "+", Sub => "-", Mul => "*", Div => "/",
            Min => "min", Max => "max", Pow => "pow",
            Lt => "lt", Gt => "gt", Le => "le", Ge => "ge",
            Fold => "fold", Acc => "acc", Elem => "elem",
            Fma => "fma", Select => "select",
        }
    }
}
