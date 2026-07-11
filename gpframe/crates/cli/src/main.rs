//! dge — argv dispatcher over the cli library.

const USAGE: &str = "\
dge — deductive-gp-engine
  dge audit    <src-dir>                -> s = Term_p-extractable share (§9 pre-R0 gate)
  dge extract  <file.rs> <fn_name>       -> Rust fn -> Term_p s-expr (gate it!)
  dge discharge [--artifacts <dir>]     -> O1: prove every Dec rule via Z3
  dge refactor <fn.sexpr> [--eps]       -> (P', certificate)
  dge gentest  <fn.sexpr> <phi.rs>      -> pinned suite + adequacy    [R3 TODO]
  dge debug    <fn.sexpr> <phi.rs>      -> minimal CE + Ochiai aid    [R4 TODO]
  dge calib                             -> per-op cost tables         [R7 TODO]
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("audit") => cli::audit::run(&args[2..]),
        Some("extract") => cli::extract::run(&args[2..]),
        Some("discharge") => cli::discharge::run(&args[2..]),
        Some("refactor") => cli::refactor::run(&args[2..]),
        Some("gentest") => cli::gentest::run(&args[2..]),
        Some("debug") => cli::debug::run(&args[2..]),
        Some("calib") => cli::calib::run(&args[2..]),
        _ => print!("{USAGE}"),
    }
}
