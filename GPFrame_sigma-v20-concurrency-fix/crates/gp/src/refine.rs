//! Constant refinement — Nelder-Mead on the const pool post-convergence
//! (v1 M3). Shape is FROZEN; only `Term::consts` moves — which is exactly
//! why consts live out-of-line in the arena.

use term::Term;

pub struct NmParams {
    pub max_iters: u64,
    pub tol: f64,
}

impl Default for NmParams {
    fn default() -> Self { Self { max_iters: 500, tol: 1e-12 } }
}

/// Minimize `objective(consts)` over the const pool; returns the refined
/// term and the final objective value. Standard Nelder-Mead
/// (reflect 1, expand 2, contract 0.5, shrink 0.5).
pub fn refine(t: &Term, objective: &dyn Fn(&Term) -> f64, p: &NmParams) -> (Term, f64) {
    let k = t.consts.len();
    if k == 0 {
        return (t.clone(), objective(t));
    }
    let with = |c: &[f64]| -> Term {
        let mut t2 = t.clone();
        t2.consts.copy_from_slice(c);
        t2
    };
    let f = |c: &[f64]| objective(&with(c));

    // initial simplex: x0 plus per-axis perturbations
    let x0 = t.consts.clone();
    let mut simplex: Vec<(Vec<f64>, f64)> = Vec::with_capacity(k + 1);
    simplex.push((x0.clone(), f(&x0)));
    for i in 0..k {
        let mut xi = x0.clone();
        xi[i] += if xi[i] != 0.0 { 0.05 * xi[i] } else { 0.25 };
        let fi = f(&xi);
        simplex.push((xi, fi));
    }

    for _ in 0..p.max_iters {
        simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
        if (simplex[k].1 - simplex[0].1).abs() < p.tol {
            break;
        }
        // centroid of all but worst
        let mut cen = vec![0.0; k];
        for (x, _) in &simplex[..k] {
            for i in 0..k { cen[i] += x[i] / k as f64; }
        }
        let worst = simplex[k].clone();
        let refl: Vec<f64> = (0..k).map(|i| cen[i] + (cen[i] - worst.0[i])).collect();
        let fr = f(&refl);

        if fr < simplex[0].1 {
            let exp: Vec<f64> = (0..k).map(|i| cen[i] + 2.0 * (cen[i] - worst.0[i])).collect();
            let fe = f(&exp);
            simplex[k] = if fe < fr { (exp, fe) } else { (refl, fr) };
        } else if fr < simplex[k - 1].1 {
            simplex[k] = (refl, fr);
        } else {
            let con: Vec<f64> = (0..k).map(|i| cen[i] + 0.5 * (worst.0[i] - cen[i])).collect();
            let fc = f(&con);
            if fc < worst.1 {
                simplex[k] = (con, fc);
            } else {
                // shrink toward best
                let best = simplex[0].0.clone();
                for e in simplex.iter_mut().skip(1) {
                    for i in 0..k { e.0[i] = best[i] + 0.5 * (e.0[i] - best[i]); }
                    e.1 = f(&e.0);
                }
            }
        }
    }
    simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
    (with(&simplex[0].0), simplex[0].1)
}
