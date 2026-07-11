//! Σ → CLIF lowering. [L1 | R7] Cranelift is NOT trusted base — O7 keeps it
//! refutable (see install.rs). Lowering policy:
//!
//! * native CLIF: + − × ÷ neg abs sqrt floor ceil, select (fcmp NE + select)
//! * extern "C" wrappers over the SAME Rust methods the interpreter calls:
//!   sin cos tan exp ln pow, fma (mul_add), min, max — bitwise agreement
//!   with the oracle BY CONSTRUCTION, libm identity pinned by O8.
//!
//! `naive_min_max` (default OFF) lowers min/max to CLIF fmin/fmax instead —
//! whose NaN semantics DIFFER from Rust's (fmin propagates NaN; f64::min
//! returns the other operand). It exists so the O7 differential gate can be
//! demonstrated catching a real compiler-semantics mismatch; see the
//! `o7_catches_naive_fmin` test.
//!
//! SPEC CORRECTION (R2, gate-refuted): the "≤1 ULP" relaxation claimed in
//! v2.1 §1 for fma contraction is FALSE under cancellation (a*b ≈ -c ⇒
//! unbounded ULP difference). If contraction is ON, the O7 gate must use
//! Metric::fma_mixed() and the certificate carries both flag and metric.
//! Bitwise honesty exists only with contraction OFF (we lower Fma via the
//! mul_add wrapper — fused semantics, same as interp — so the SYMBOL Fma is
//! bitwise-exact; the flag concerns contracting (+ (* a b) c) PATTERNS).

use cranelift_codegen::ir::{condcodes::FloatCC, types, AbiParam, InstBuilder, MemFlags, Value};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::HashMap;
use term::{Op, Term};

/// Compiled signature. SAFETY: caller must pass a pointer to at least
/// `term.arity()` contiguous f64s.
pub type RawFn = unsafe extern "C" fn(*const f64) -> f64;

pub struct LowerConfig {
    /// See module doc — pattern-level contraction switch (v2.1 §1, corrected).
    pub fma_contraction: bool,
    /// Deliberately-wrong min/max lowering for the O7 demonstration.
    pub naive_min_max: bool,
}

impl Default for LowerConfig {
    fn default() -> Self {
        Self { fma_contraction: false, naive_min_max: false }
    }
}

#[derive(Debug)]
pub enum LowerError {
    Backend(String),
}

// ---- extern "C" oracle-identical wrappers --------------------------------
extern "C" fn w_sin(x: f64) -> f64 { x.sin() }
extern "C" fn w_cos(x: f64) -> f64 { x.cos() }
extern "C" fn w_tan(x: f64) -> f64 { x.tan() }
extern "C" fn w_exp(x: f64) -> f64 { x.exp() }
extern "C" fn w_ln(x: f64) -> f64 { x.ln() }
extern "C" fn w_pow(a: f64, b: f64) -> f64 { a.powf(b) }
extern "C" fn w_min(a: f64, b: f64) -> f64 { a.min(b) }
extern "C" fn w_max(a: f64, b: f64) -> f64 { a.max(b) }
extern "C" fn w_fma(a: f64, b: f64, c: f64) -> f64 { a.mul_add(b, c) }

const WRAPPERS: &[(&str, usize, *const u8)] = &[
    ("w_sin", 1, w_sin as *const u8),
    ("w_cos", 1, w_cos as *const u8),
    ("w_tan", 1, w_tan as *const u8),
    ("w_exp", 1, w_exp as *const u8),
    ("w_ln", 1, w_ln as *const u8),
    ("w_pow", 2, w_pow as *const u8),
    ("w_min", 2, w_min as *const u8),
    ("w_max", 2, w_max as *const u8),
    ("w_fma", 3, w_fma as *const u8),
];

/// The compiled artifact — keeps the JITModule alive (code memory owner).
pub struct Compiled {
    pub raw: RawFn,
    _module: JITModule,
}

pub fn lower(t: &Term, cfg: &LowerConfig) -> Result<Compiled, LowerError> {
    let e = |s: String| LowerError::Backend(s);

    let mut flags = settings::builder();
    flags.set("use_colocated_libcalls", "false").map_err(|x| e(x.to_string()))?;
    flags.set("is_pic", "false").map_err(|x| e(x.to_string()))?;
    flags.set("opt_level", "speed").map_err(|x| e(x.to_string()))?;
    let isa = cranelift_native::builder()
        .map_err(|x| e(x.to_string()))?
        .finish(settings::Flags::new(flags))
        .map_err(|x| e(x.to_string()))?;

    let mut jb = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    for (name, _, ptr) in WRAPPERS {
        jb.symbol(*name, *ptr);
    }
    let mut module = JITModule::new(jb);

    // declare wrapper imports
    let mut wrapper_ids: HashMap<&str, FuncId> = HashMap::new();
    for (name, arity, _) in WRAPPERS {
        let mut sig = module.make_signature();
        for _ in 0..*arity {
            sig.params.push(AbiParam::new(types::F64));
        }
        sig.returns.push(AbiParam::new(types::F64));
        let id = module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|x| e(x.to_string()))?;
        wrapper_ids.insert(*name, id);
    }

    // the term's function: fn(ptr: i64) -> f64
    let mut ctx = module.make_context();
    let ptr_ty = module.target_config().pointer_type();
    ctx.func.signature.params.push(AbiParam::new(ptr_ty));
    ctx.func.signature.returns.push(AbiParam::new(types::F64));

    let mut fbx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbx);
        let block = b.create_block();
        b.append_block_params_for_function_params(block);
        b.switch_to_block(block);
        b.seal_block(block);
        let ptr = b.block_params(block)[0];

        let call1 = |b: &mut FunctionBuilder, m: &mut JITModule, name: &str, args: &[Value]| {
            let fref = m.declare_func_in_func(wrapper_ids[name], b.func);
            let call = b.ins().call(fref, args);
            b.inst_results(call)[0]
        };

        // single pass over the arena (topological invariant)
        let mut val: Vec<Value> = Vec::with_capacity(t.len());
        for n in &t.nodes {
            let v = match n.op {
                Op::Const => b.ins().f64const(t.consts[n.a as usize]),
                Op::Var => b.ins().load(
                    types::F64,
                    MemFlags::trusted(),
                    ptr,
                    (n.a as i32) * 8,
                ),
                Op::Neg => { let a = val[n.a as usize]; b.ins().fneg(a) }
                Op::Abs => { let a = val[n.a as usize]; b.ins().fabs(a) }
                Op::Sqrt => { let a = val[n.a as usize]; b.ins().sqrt(a) }
                Op::Floor => { let a = val[n.a as usize]; b.ins().floor(a) }
                Op::Ceil => { let a = val[n.a as usize]; b.ins().ceil(a) }
                Op::Add => { let (a, c) = (val[n.a as usize], val[n.b as usize]); b.ins().fadd(a, c) }
                Op::Sub => { let (a, c) = (val[n.a as usize], val[n.b as usize]); b.ins().fsub(a, c) }
                Op::Mul => { let (a, c) = (val[n.a as usize], val[n.b as usize]); b.ins().fmul(a, c) }
                Op::Div => { let (a, c) = (val[n.a as usize], val[n.b as usize]); b.ins().fdiv(a, c) }
                Op::Min if cfg.naive_min_max => {
                    let (a, c) = (val[n.a as usize], val[n.b as usize]);
                    b.ins().fmin(a, c) // WRONG on NaN vs Rust — O7 bait
                }
                Op::Max if cfg.naive_min_max => {
                    let (a, c) = (val[n.a as usize], val[n.b as usize]);
                    b.ins().fmax(a, c)
                }
                Op::Min => { let args = [val[n.a as usize], val[n.b as usize]]; call1(&mut b, &mut module, "w_min", &args) }
                Op::Max => { let args = [val[n.a as usize], val[n.b as usize]]; call1(&mut b, &mut module, "w_max", &args) }
                Op::Sin => { let args = [val[n.a as usize]]; call1(&mut b, &mut module, "w_sin", &args) }
                Op::Cos => { let args = [val[n.a as usize]]; call1(&mut b, &mut module, "w_cos", &args) }
                Op::Tan => { let args = [val[n.a as usize]]; call1(&mut b, &mut module, "w_tan", &args) }
                Op::Exp => { let args = [val[n.a as usize]]; call1(&mut b, &mut module, "w_exp", &args) }
                Op::Ln => { let args = [val[n.a as usize]]; call1(&mut b, &mut module, "w_ln", &args) }
                Op::Pow => { let args = [val[n.a as usize], val[n.b as usize]]; call1(&mut b, &mut module, "w_pow", &args) }
                Op::Fma => { let args = [val[n.a as usize], val[n.b as usize], val[n.c as usize]]; call1(&mut b, &mut module, "w_fma", &args) }
                Op::Select => {
                    let (c, th, el) = (val[n.a as usize], val[n.b as usize], val[n.c as usize]);
                    let zero = b.ins().f64const(0.0);
                    // FloatCC::NotEqual = unordered-or-unequal ⇒ NaN cond
                    // takes the then-branch, matching interp's `c != 0.0`.
                    let cond = b.ins().fcmp(FloatCC::NotEqual, c, zero);
                    b.ins().select(cond, th, el)
                }
            };
            val.push(v);
        }
        let ret = val[t.root as usize];
        b.ins().return_(&[ret]);
        b.finalize();
    }

    let fid = module
        .declare_function("dge_term", Linkage::Export, &ctx.func.signature)
        .map_err(|x| e(x.to_string()))?;
    module.define_function(fid, &mut ctx).map_err(|x| e(x.to_string()))?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().map_err(|x| e(x.to_string()))?;

    let code = module.get_finalized_function(fid);
    // SAFETY: signature matches the one we just built.
    let raw: RawFn = unsafe { std::mem::transmute::<*const u8, RawFn>(code) };
    Ok(Compiled { raw, _module: module })
}
