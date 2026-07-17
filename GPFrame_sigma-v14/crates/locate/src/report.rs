//! Single debugging artifact: minimal CE + ranked nodes.

use harness::CounterExample;

pub struct DebugReport {
    pub minimal_ce: CounterExample,
    /// (node_id, ochiai score), descending. AID, not verdict — the report
    /// text must carry that framing verbatim.
    pub ranking: Vec<(usize, f64)>,
}

impl DebugReport {
    pub fn render(&self) -> String {
        let mut s = String::from("== debug report ==\n");
        s += &format!("minimal counterexample (T3-minimal): {:?}\n", self.minimal_ce.minimal_env);
        s += "suspiciousness ranking (Ochiai — ranking aid, not a verdict):\n";
        for (node, score) in self.ranking.iter().take(10) {
            s += &format!("  node {node:>4}  {score:.3}\n");
        }
        s
    }
}
