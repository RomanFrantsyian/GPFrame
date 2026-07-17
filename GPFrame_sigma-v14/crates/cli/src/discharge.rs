//! `dge discharge` — O1 discharge of the Dec rule table via Z3, artifacts
//! to disk. Run once per rule-set change; refactor refuses without it.

use rules::smt::{discharge_all, Z3Cli};

pub fn run(args: &[String]) {
    let dir = args.iter().position(|a| a == "--artifacts")
        .and_then(|i| args.get(i + 1)).map(String::as_str)
        .unwrap_or("artifacts/o1");
    if !Z3Cli::available() {
        eprintln!("z3 binary not found — install z3 to discharge O1 obligations");
        return;
    }
    let mut z3 = Z3Cli::new(dir);
    let (proved, rejected, unknown) = discharge_all(&rules::r_dec::table(), &mut z3);
    for p in &proved { println!("UNSAT  {p}  (artifact stored)"); }
    for r in &rejected { println!("SAT    {r}  — RULE REJECTED (counterexample exists)"); }
    for u in &unknown { println!("UNKNOWN {u} — route to r_sem proof or r_approx"); }
    println!("--\n{} proved, {} rejected, {} unknown -> {dir}",
        proved.len(), rejected.len(), unknown.len());
}
