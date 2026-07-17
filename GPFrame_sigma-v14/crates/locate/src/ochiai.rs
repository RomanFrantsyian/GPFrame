//! Ochiai suspiciousness: s(node) = ef / sqrt((ef+nf)(ef+ep)).
//! A conditional-probability estimator of P(node faulty | failures observed).

use crate::spectrum::Spectrum;

pub fn rank(s: &Spectrum) -> Vec<(usize, f64)> {
    let mut out: Vec<(usize, f64)> = (0..s.n_nodes)
        .map(|j| {
            let (ef, ep, nf, _np) = s.counts(j);
            let denom = (((ef + nf) * (ef + ep)) as f64).sqrt();
            let score = if denom == 0.0 { 0.0 } else { ef as f64 / denom };
            (j, score)
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}
