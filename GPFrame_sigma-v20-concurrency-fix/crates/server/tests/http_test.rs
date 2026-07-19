//! The isolation boundary, tested over a REAL socket: a live listener on
//! an ephemeral port, raw HTTP/1.1 requests from a client thread, JSON
//! back. What third-party microservices will actually do.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn spawn_server() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve(l));
    addr
}

fn post(addr: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(s, "POST {path} HTTP/1.1\r\nHost: dge\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()).unwrap();
    read_resp(s)
}

fn get(addr: &str, path: &str) -> (u16, serde_json::Value) {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: dge\r\n\r\n").unwrap();
    read_resp(s)
}

fn read_resp(mut s: TcpStream) -> (u16, serde_json::Value) {
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    let code: u16 = buf.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = buf.split("\r\n\r\n").nth(1).unwrap();
    (code, serde_json::from_str(body).unwrap())
}

#[test]
fn version_and_alphabet_answer() {
    let a = spawn_server();
    let (c, v) = get(&a, "/v1/version");
    assert_eq!(c, 200);
    assert_eq!(v["sigma"], "v1.7");
    let (c, v) = get(&a, "/v1/alphabet");
    assert_eq!(c, 200);
    let ops = v["ops"].as_array().unwrap();
    assert!(ops.iter().any(|o| o["name"] == "rnd32" && o["arity"] == 1));
    assert!(v["ext_ops"]["syntax"].as_str().unwrap().contains("ext:"));
    assert!(ops.iter().any(|o| o["name"] == "fold" && o["arity"] == 2));
}

#[test]
fn extract_eval_certify_round_trip_over_the_wire() {
    let a = spawn_server();
    // extract: both doors admit, sexprs come back
    let (c, v) = post(&a, "/v1/extract", &serde_json::json!({
        "source": "#[no_mangle]\npub fn twice(x: f64) -> f64 { x * 2.0 }",
        "fn_name": "twice"
    }).to_string());
    assert_eq!(c, 200);
    let doors = v["doors"].as_array().unwrap();
    assert_eq!(doors.len(), 2);
    assert!(doors.iter().all(|d| d["admitted"] == true));
    let sx = doors[0]["sexpr"].as_str().unwrap().to_string();

    // eval the sexpr remotely; bits must equal local 21*2
    let (c, v) = post(&a, "/v1/eval", &serde_json::json!({
        "sexpr": sx, "env": [21.0]
    }).to_string());
    assert_eq!(c, 200);
    assert_eq!(v["bits"], format!("{:#018x}", 42.0f64.to_bits()));

    // certify: cross-door gate promotes, emitted code arrives
    let (c, v) = post(&a, "/v1/certify", &serde_json::json!({
        "source": "#[no_mangle]\npub fn twice(x: f64) -> f64 { x * 2.0 }",
        "fn_name": "twice", "seed": 7
    }).to_string());
    assert_eq!(c, 200);
    assert_eq!(v["report"]["promoted"], true);
    assert_eq!(v["report"]["certificate"]["n"], 10_000);
    assert!(v["report"]["emitted"].as_str().unwrap().contains("fn twice"));
}

#[test]
fn gate_refutes_a_lying_candidate_over_the_wire() {
    let a = spawn_server();
    let (c, v) = post(&a, "/v1/gate", &serde_json::json!({
        "candidate": "(+ (var 0) 1.0)",
        "reference": "(* (var 0) 2.0)",
        "fn_name": "liar"
    }).to_string());
    assert_eq!(c, 200);
    assert_eq!(v["promoted"], false);
    assert!(v["counterexample"]["minimal_env"].is_array());
}

#[test]
fn refusals_cross_the_wire_with_trial_buckets() {
    let a = spawn_server();
    let (c, v) = post(&a, "/v1/extract", &serde_json::json!({
        "source": "pub fn s(t: f32) -> f32 { t.sin() }",
        "fn_name": "s", "door": "syn"
    }).to_string());
    assert_eq!(c, 200);
    let d = &v["doors"][0];
    assert_eq!(d["admitted"], false);
    assert!(d["refusal"].as_str().unwrap().contains("innocuous"));
    assert!(d["class"].is_string());
}

#[test]
fn malformed_requests_get_400_not_500() {
    let a = spawn_server();
    let (c, _) = post(&a, "/v1/extract", "{not json");
    assert_eq!(c, 400);
    let (c, v) = post(&a, "/v1/eval", r#"{"env": [1.0]}"#);
    assert_eq!(c, 400);
    assert!(v["error"].as_str().unwrap().contains("sexpr"));
    let (c, _) = get(&a, "/v1/nope");
    assert_eq!(c, 404);
}

#[test]
fn gate_refuses_honestly_on_unregistered_ext_op() {
    // v1.7: an ext op the server hasn't registered yields a `refused`
    // report over the wire, distinct from a bit-level `refuted`.
    let a = spawn_server();
    let (c, v) = post(&a, "/v1/gate", &serde_json::json!({
        "candidate": "(ext:totally_unregistered_op (var 0))",
        "reference": "(var 0)",
        "fn_name": "ghost"
    }).to_string());
    assert_eq!(c, 200);
    assert_eq!(v["promoted"], false);
    assert!(v["refused"].as_str().unwrap().contains("not registered"));
}
