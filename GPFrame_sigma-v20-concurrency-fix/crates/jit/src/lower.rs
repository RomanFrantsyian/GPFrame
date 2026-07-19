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

/// Compiled signature (Σ v1.2): scalars pointer, array of sequence base
/// pointers (one per Elem index, all sequences the same length), shared
/// length. SAFETY: scalars must cover `arity()`, seq_ptrs must cover
/// `seq_count()` valid slices of `seq_len` f64s each.
pub type RawFn = unsafe extern "C" fn(*const f64, *const *const f64, i64) -> f64;

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
extern "C" fn w_exp2(x: f64) -> f64 { x.exp2() }
extern "C" fn w_ln(x: f64) -> f64 { x.ln() }
extern "C" fn w_pow(a: f64, b: f64) -> f64 { a.powf(b) }
extern "C" fn w_min(a: f64, b: f64) -> f64 { a.min(b) }
extern "C" fn w_max(a: f64, b: f64) -> f64 { a.max(b) }
extern "C" fn w_fma(a: f64, b: f64, c: f64) -> f64 { a.mul_add(b, c) }

// Σ-ext trampolines: the first param is a leaked `*const ExtFn` (the
// plugin closure, resolved and pinned at LOWER time — same registry Arc
// the interpreter dispatches through, so JIT and interp share ONE
// semantics by construction; O7 still differentials them). The leak is
// deliberate and bounded: one small allocation per ext node per install,
// alive as long as the compiled code may run.
extern "C" fn w_ext1(f: *const term::ext::ExtFn, x: f64) -> f64 {
    unsafe { (*f)(&[x]) }
}
extern "C" fn w_ext2(f: *const term::ext::ExtFn, x: f64, y: f64) -> f64 {
    unsafe { (*f)(&[x, y]) }
}

const WRAPPERS: &[(&str, usize, *const u8)] = &[
    ("w_sin", 1, w_sin as *const u8),
    ("w_cos", 1, w_cos as *const u8),
    ("w_tan", 1, w_tan as *const u8),
    ("w_exp", 1, w_exp as *const u8),
    ("w_exp2", 1, w_exp2 as *const u8),
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


/// Shared instruction selection for every non-leaf, non-fold op — used
/// identically by the straight-line pass and inside fold loop bodies, so
/// the two contexts cannot drift semantically.
fn emit_op(
    b: &mut FunctionBuilder,
    module: &mut JITModule,
    wrapper_ids: &HashMap<&str, FuncId>,
    cfg: &LowerConfig,
    _t: &Term,
    n: &term::Node,
    val: &[Value],
) -> Value {
    let call1 = |b: &mut FunctionBuilder, m: &mut JITModule, name: &str, args: &[Value]| {
        let fref = m.declare_func_in_func(wrapper_ids[name], b.func);
        let call = b.ins().call(fref, args);
        b.inst_results(call)[0]
    };
    match n.op {
                Op::Neg => { let a = val[n.a as usize]; b.ins().fneg(a) }
                Op::Abs => { let a = val[n.a as usize]; b.ins().fabs(a) }
                Op::Sqrt => { let a = val[n.a as usize]; b.ins().sqrt(a) }
                Op::Floor => { let a = val[n.a as usize]; b.ins().floor(a) }
                Op::Ceil => { let a = val[n.a as usize]; b.ins().ceil(a) }
                Op::Add => { let (a, c) = (val[n.a as usize], val[n.b as usize]); b.ins().fadd(a, c) }
                Op::Sub => { let (a, c) = (val[n.a as usize], val[n.b as usize]); b.ins().fsub(a, c) }
                Op::Mul => { let (a, c) = (val[n.a as usize], val[n.b as usize]); b.ins().fmul(a, c) }
                Op::Div => { let (a, c) = (val[n.a as usize], val[n.b as usize]); b.ins().fdiv(a, c) }
                Op::Lt | Op::Gt | Op::Le | Op::Ge | Op::Eq | Op::Ne => {
                    let (a, c) = (val[n.a as usize], val[n.b as usize]);
                    let cc = match n.op {
                        Op::Lt => FloatCC::LessThan,
                        Op::Gt => FloatCC::GreaterThan,
                        Op::Le => FloatCC::LessThanOrEqual,
                        // Rust `==` is ORDERED equal (NaN ⇒ false);
                        // Rust `!=` is UNORDERED not-equal (NaN ⇒ true) —
                        // CLIF Equal/NotEqual are exactly that pair.
                        Op::Eq => FloatCC::Equal,
                        Op::Ne => FloatCC::NotEqual,
                        _ => FloatCC::GreaterThanOrEqual,
                    }; // Lt/Gt/Le/Ge ORDERED: false on NaN — Rust semantics
                    let cond = b.ins().fcmp(cc, a, c);
                    let one = b.ins().f64const(1.0);
                    let zero = b.ins().f64const(0.0);
                    b.ins().select(cond, one, zero)
                }
                Op::Min if cfg.naive_min_max => {
                    let (a, c) = (val[n.a as usize], val[n.b as usize]);
                    b.ins().fmin(a, c) // WRONG on NaN vs Rust — O7 bait
                }
                Op::Max if cfg.naive_min_max => {
                    let (a, c) = (val[n.a as usize], val[n.b as usize]);
                    b.ins().fmax(a, c)
                }
                Op::Min => { let args = [val[n.a as usize], val[n.b as usize]]; call1(b, module, "w_min", &args) }
                Op::Max => { let args = [val[n.a as usize], val[n.b as usize]]; call1(b, module, "w_max", &args) }
                Op::Sin => { let args = [val[n.a as usize]]; call1(b, module, "w_sin", &args) }
                Op::Cos => { let args = [val[n.a as usize]]; call1(b, module, "w_cos", &args) }
                Op::Tan => { let args = [val[n.a as usize]]; call1(b, module, "w_tan", &args) }
                Op::Exp => { let args = [val[n.a as usize]]; call1(b, module, "w_exp", &args) }
                Op::Exp2 => { let args = [val[n.a as usize]]; call1(b, module, "w_exp2", &args) }
                Op::Ln => { let args = [val[n.a as usize]]; call1(b, module, "w_ln", &args) }
                // Σ-ext: resolve the plugin closure NOW (lowering already
                // failed with an honest message if unregistered — see the
                // pre-scan in lower()), leak it, pass as a pointer const.
                Op::Ext1 | Op::Ext2 => {
                    let idx = if n.op == Op::Ext1 { n.b } else { n.c };
                    let name = &_t.exts[idx as usize];
                    let def = term::ext::lookup(name)
                        .expect("pre-scanned in lower(): registered");
                    let leaked: *const term::ext::ExtFn =
                        Box::into_raw(Box::new(def.f.clone()));
                    let ptr_ty = module.target_config().pointer_type();
                    let p = b.ins().iconst(ptr_ty, leaked as i64);
                    if n.op == Op::Ext1 {
                        let args = [p, val[n.a as usize]];
                        call1(b, module, "w_ext1", &args)
                    } else {
                        let args = [p, val[n.a as usize], val[n.b as usize]];
                        call1(b, module, "w_ext2", &args)
                    }
                }
                // Rnd32: fdemote(f64→f32) then fpromote back — IEEE
                // round-to-nearest-even both directions, the same bits the
                // interpreter's `(x as f32) as f64` produces; pure CLIF,
                // no libm indirection.
                Op::Rnd32 => {
                    let narrow = b.ins().fdemote(types::F32,
                        val[n.a as usize]);
                    b.ins().fpromote(types::F64, narrow)
                }
                Op::Pow => { let args = [val[n.a as usize], val[n.b as usize]]; call1(b, module, "w_pow", &args) }
                Op::Fma => { let args = [val[n.a as usize], val[n.b as usize], val[n.c as usize]]; call1(b, module, "w_fma", &args) }
                Op::Select => {
                    let (c, th, el) = (val[n.a as usize], val[n.b as usize], val[n.c as usize]);
                    let zero = b.ins().f64const(0.0);
                    // FloatCC::NotEqual = unordered-or-unequal ⇒ NaN cond
                    // takes the then-branch, matching interp's `c != 0.0`.
                    let cond = b.ins().fcmp(FloatCC::NotEqual, c, zero);
                    b.ins().select(cond, th, el)
                }
        Op::Const | Op::Var | Op::Acc | Op::Elem | Op::Len | Op::Fold =>
            unreachable!("leaves handled by callers"),
    }
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
    jb.symbol("w_ext1", w_ext1 as *const u8);
    jb.symbol("w_ext2", w_ext2 as *const u8);
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
    // ext trampoline imports: (ptr, f64…) -> f64
    {
        let ptr_ty = module.target_config().pointer_type();
        for (name, fargs) in [("w_ext1", 1), ("w_ext2", 2)] {
            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(ptr_ty));
            for _ in 0..fargs { sig.params.push(AbiParam::new(types::F64)); }
            sig.returns.push(AbiParam::new(types::F64));
            let id = module.declare_function(name, Linkage::Import, &sig)
                .map_err(|x| e(x.to_string()))?;
            wrapper_ids.insert(name, id);
        }
    }

    // the term's function: fn(scalars, seq_ptrs, seq_len) -> f64
    let mut ctx = module.make_context();
    let ptr_ty = module.target_config().pointer_type();
    ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // scalars
    ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // seq base ptrs
    ctx.func.signature.params.push(AbiParam::new(types::I64)); // shared len
    ctx.func.signature.returns.push(AbiParam::new(types::F64));

    // Σ-ext pre-scan: refuse honestly BEFORE codegen if any ext op is
    // unregistered (emit_op then safely assumes resolution succeeds)
    term::ext::tags_for(&t.exts).map_err(|s| e(s))?;

    let owners = t.fold_owners().map_err(|s| e(format!("fold validation: {s}")))?;
    let mut owned_of: Vec<Vec<usize>> = vec![Vec::new(); t.len()];
    for (i, o) in owners.iter().enumerate() {
        if let Some(f) = o { owned_of[*f as usize].push(i); }
    }

    let mut fbx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbx);
        let block = b.create_block();
        b.append_block_params_for_function_params(block);
        b.switch_to_block(block);
        b.seal_block(block);
        let ptr = b.block_params(block)[0];
        let seq_ptrs = b.block_params(block)[1];
        let seq_len = b.block_params(block)[2];
        // hoist sequence base pointers (entry dominates every loop)
        let ptr_ty = b.func.dfg.value_type(ptr);
        let seq_base: Vec<Value> = (0..t.seq_count())
            .map(|k| b.ins().load(ptr_ty, MemFlags::trusted(), seq_ptrs,
                (k * std::mem::size_of::<usize>()) as i32))
            .collect();

        // single pass over the arena (topological invariant); fold-owned
        // nodes are skipped here and re-emitted inside their loop body
        let dummy = b.ins().f64const(0.0);
        let mut val: Vec<Value> = vec![dummy; t.len()];
        for (idx, n) in t.nodes.iter().enumerate() {
            if owners[idx].is_some() { continue; }
            if n.op == Op::Fold {
                // -------- Σ v1.2 loop codegen --------
                let init_v = val[n.a as usize];
                let loop_head = b.create_block();
                b.append_block_param(loop_head, types::F64); // acc
                b.append_block_param(loop_head, types::I64); // i
                let body_blk = b.create_block();
                let exit_blk = b.create_block();
                b.append_block_param(exit_blk, types::F64); // result

                let zero_i = b.ins().iconst(types::I64, 0);
                b.ins().jump(loop_head, &[init_v, zero_i]);

                b.switch_to_block(loop_head);
                let acc_p = b.block_params(loop_head)[0];
                let i_p = b.block_params(loop_head)[1];
                let more = b.ins().icmp(
                    cranelift_codegen::ir::condcodes::IntCC::SignedLessThan, i_p, seq_len);
                b.ins().brif(more, body_blk, &[], exit_blk, &[acc_p]);

                b.switch_to_block(body_blk);
                b.seal_block(body_blk);
                let byte_off = b.ins().imul_imm(i_p, 8);
                for &j in &owned_of[idx] {
                    let bn = &t.nodes[j];
                    val[j] = match bn.op {
                        Op::Const => b.ins().f64const(t.consts[bn.a as usize]),
                        Op::Var => b.ins().load(types::F64, MemFlags::trusted(), ptr,
                            (bn.a as i32) * 8),
                        Op::Acc => acc_p,
                        Op::Elem => {
                            let addr = b.ins().iadd(seq_base[bn.a as usize], byte_off);
                            b.ins().load(types::F64, MemFlags::trusted(), addr, 0)
                        }
                        // ABI: all sequences share one length (asserted at
                        // the door) — Len(k) is that length for every k.
                        Op::Len => b.ins().fcvt_from_uint(types::F64, seq_len),
                        _ => emit_op(&mut b, &mut module, &wrapper_ids, cfg, t, bn, &val),
                    };
                }
                let acc_next = val[n.b as usize];
                let i_next = b.ins().iadd_imm(i_p, 1);
                b.ins().jump(loop_head, &[acc_next, i_next]);
                b.seal_block(loop_head);

                b.switch_to_block(exit_blk);
                b.seal_block(exit_blk);
                val[idx] = b.block_params(exit_blk)[0];
                continue;
            }
            let v = match n.op {
                Op::Const => b.ins().f64const(t.consts[n.a as usize]),
                Op::Var => b.ins().load(
                    types::F64,
                    MemFlags::trusted(),
                    ptr,
                    (n.a as i32) * 8,
                ),
                Op::Len => b.ins().fcvt_from_uint(types::F64, seq_len),
                // orphan (unreachable) binders are tolerated by fold_owners;
                // never consumed, so a 0.0 placeholder keeps the single
                // arena pass total. Fold is impossible here (handled above).
                Op::Acc | Op::Elem => b.ins().f64const(0.0),
                Op::Fold => unreachable!("handled above"),
                _ => emit_op(&mut b, &mut module, &wrapper_ids, cfg, t, n, &val),
            };
            val[idx] = v;
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
