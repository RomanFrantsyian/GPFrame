//! `dge-serve [addr]` — default 127.0.0.1:7464 ("DGE" on a phone pad-ish).
//! Binds loopback by default: exposing this beyond localhost is a
//! DEPLOYMENT decision (put your own auth/transport in front; the API
//! itself is unauthenticated by design — see docs/API.md §security).
fn main() {
    let addr = std::env::args().nth(1)
        .unwrap_or_else(|| "127.0.0.1:7464".into());
    let l = std::net::TcpListener::bind(&addr)
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    eprintln!("dge-serve listening on http://{addr}  (contract: docs/API.md)");
    server::serve(l);
}
