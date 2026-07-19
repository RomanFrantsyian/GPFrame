//! dge-serve — DGE behind a network boundary.
//!
//! Hand-rolled HTTP/1.1 over std::net (one thread per connection; the
//! engine is stateless per request, parameterized by the request's seed).
//! The contract is docs/API.md; the design rule is the SDK's: the
//! verified state never crosses the wire. Remote callers receive sexprs,
//! refusals (with trial buckets), certificates-as-data, and emitted
//! code — arbitration happens HERE, on the server's gate.

use sdk::{Engine, ExtractRequest, GateReport};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

pub fn serve(listener: TcpListener) -> ! {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                std::thread::spawn(move || { let _ = handle(stream); });
            }
            Err(_) => continue,
        }
    }
}

fn handle(mut s: TcpStream) -> std::io::Result<()> {
    // minimal HTTP/1.1: request line + headers to \r\n\r\n, then
    // Content-Length body
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = s.read(&mut tmp)?;
        if n == 0 { return Ok(()); }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = find(&buf, b"\r\n\r\n") { break p + 4; }
        if buf.len() > 64 * 1024 { return respond(&mut s, 431, json!({"error": "headers too large"})); }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let clen: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    if clen > 4 * 1024 * 1024 {
        return respond(&mut s, 413, json!({"error": "body too large"}));
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < clen {
        let n = s.read(&mut tmp)?;
        if n == 0 { break; }
        body.extend_from_slice(&tmp[..n]);
    }
    let (code, out) = route(method, path, &body);
    respond(&mut s, code, out)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn respond(s: &mut TcpStream, code: u16, v: Value) -> std::io::Result<()> {
    let body = v.to_string();
    let status = match code {
        200 => "OK", 400 => "Bad Request", 404 => "Not Found",
        413 => "Payload Too Large", 431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    write!(s, "HTTP/1.1 {code} {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len())?;
    s.flush()
}

// --------------------------------------------------------------- routes --

pub fn route(method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    match (method, path) {
        ("GET", "/v1/version") => (200, json!({
            "engine": "deductive-gp-engine",
            "sigma": "v1.7",
            "api": "v1",
        })),
        ("GET", "/v1/alphabet") => (200, alphabet()),
        ("POST", "/v1/extract") => with_json(body, extract),
        ("POST", "/v1/eval")    => with_json(body, eval),
        ("POST", "/v1/gate")    => with_json(body, gate),
        ("POST", "/v1/certify") => with_json(body, certify),
        _ => (404, json!({"error": "no such endpoint", "see": "docs/API.md"})),
    }
}

fn with_json(body: &[u8], f: fn(&Value) -> (u16, Value)) -> (u16, Value) {
    match serde_json::from_slice::<Value>(body) {
        Ok(v) => f(&v),
        Err(e) => (400, json!({"error": format!("invalid JSON: {e}")})),
    }
}

fn alphabet() -> Value {
    use term::Op::*;
    let ops = [
        Const, Var, Neg, Abs, Sqrt, Floor, Ceil, Sin, Cos, Tan, Exp, Exp2,
        Ln, Rnd32, Add, Sub, Mul, Div, Min, Max, Pow, Lt, Gt, Le, Ge, Eq,
        Ne, Fold, Acc, Elem, Len, Fma, Select,
    ];
    json!({
        "sigma": "v1.7",
        "ops": ops.iter().map(|o| json!({
            "name": o.name(), "arity": o.arity(),
        })).collect::<Vec<_>>(),
        // Σ-ext (v1.7): pluggable ops resolve by NAME, not from a fixed
        // list — `(ext:<name> a [b])` is legal syntax for any arity-1/2
        // name; whether it GATES depends on server-side registration
        // (out of scope for the HTTP surface this session: extension
        // registration is an in-process SDK operation, not a wire one).
        "ext_ops": {"syntax": "(ext:<name> a) | (ext:<name> a b)",
                     "note": "resolved by name at gate time; unregistered names refuse honestly"},
    })
}

fn str_field<'a>(v: &'a Value, k: &str) -> Result<&'a str, (u16, Value)> {
    v.get(k).and_then(Value::as_str)
        .ok_or_else(|| (400, json!({"error": format!("missing string field `{k}`")})))
}

fn seed_of(v: &Value) -> u64 {
    v.get("seed").and_then(Value::as_u64).unwrap_or(0xD6E)
}

fn extract(v: &Value) -> (u16, Value) {
    let (src, f) = match (str_field(v, "source"), str_field(v, "fn_name")) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let e = Engine::new(seed_of(v));
    let req = ExtractRequest { source: src, fn_name: f };
    let outs = match v.get("door").and_then(Value::as_str) {
        Some(d) => vec![(d.to_string(), e.extract(d, &req))],
        None => e.extract_all(&req),
    };
    (200, json!({
        "fn_name": f,
        "doors": outs.iter().map(|(d, r)| match r {
            Ok(t) => json!({"door": d, "admitted": true,
                            "sexpr": sdk::sexpr::print(t),
                            "nodes": t.nodes.len()}),
            Err(r) => json!({"door": d, "admitted": false,
                             "refusal": r.0, "class": r.class()}),
        }).collect::<Vec<_>>(),
    }))
}

fn parse_env_seqs(v: &Value) -> Result<(Vec<f64>, Vec<Vec<f64>>), (u16, Value)> {
    let env: Vec<f64> = v.get("env").and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let seqs: Vec<Vec<f64>> = v.get("seqs").and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_array().map(
            |b| b.iter().filter_map(Value::as_f64).collect())).collect())
        .unwrap_or_default();
    Ok((env, seqs))
}

fn eval(v: &Value) -> (u16, Value) {
    let sx = match str_field(v, "sexpr") { Ok(s) => s, Err(e) => return e };
    let t = match sdk::sexpr::parse(sx) {
        Ok(t) => t,
        Err(e) => return (400, json!({"error": format!("sexpr: {e:?}")})),
    };
    let (env, seqs) = match parse_env_seqs(v) { Ok(x) => x, Err(e) => return e };
    let sl: Vec<&[f64]> = seqs.iter().map(|s| s.as_slice()).collect();
    let val = sdk::eval_with_seqs(&t, &env, &sl);
    // JSON has no NaN/Inf: encode specials as strings, finite as numbers
    let jval = if val.is_finite() { json!(val) } else { json!(val.to_string()) };
    (200, json!({"value": jval, "bits": format!("{:#018x}", val.to_bits())}))
}

fn gate_report_json(r: &GateReport) -> Value {
    match r {
        GateReport::Promoted { n, alpha, emitted } => json!({
            "promoted": true,
            "certificate": {
                "n": n, "alpha": alpha, "metric": "bitwise-over-mu-prime",
                "note": "claim holds for THIS server's gate run; \
                         re-gate locally before trusting remotely",
            },
            "emitted": emitted,
        }),
        GateReport::Refused(m) => json!({
            "promoted": false, "refused": m,
        }),
        GateReport::Refuted(w) => json!({
            "promoted": false,
            "counterexample": {
                "minimal_env": w.minimal_env,
                "minimal_seqs": w.minimal_seqs,
                "candidate_val": w.candidate_val.to_string(),
                "reference_val": w.reference_val.to_string(),
            },
        }),
    }
}

fn gate(v: &Value) -> (u16, Value) {
    let (c, r) = match (str_field(v, "candidate"), str_field(v, "reference")) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let (ct, rt) = match (sdk::sexpr::parse(c), sdk::sexpr::parse(r)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) =>
            return (400, json!({"error": format!("sexpr: {e:?}")})),
    };
    let name = v.get("fn_name").and_then(Value::as_str).unwrap_or("candidate");
    let e = Engine::bare(seed_of(v));
    (200, gate_report_json(&e.gate(name, ct, &rt)))
}

fn certify(v: &Value) -> (u16, Value) {
    let (src, f) = match (str_field(v, "source"), str_field(v, "fn_name")) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let e = Engine::new(seed_of(v));
    let (outs, report) = e.certify(&ExtractRequest { source: src, fn_name: f });
    (200, json!({
        "fn_name": f,
        "doors": outs.iter().map(|(d, r)| match r {
            Ok(t) => json!({"door": d, "admitted": true,
                            "sexpr": sdk::sexpr::print(t)}),
            Err(r) => json!({"door": d, "admitted": false,
                             "refusal": r.0, "class": r.class()}),
        }).collect::<Vec<_>>(),
        "report": report.as_ref().map(gate_report_json),
    }))
}
