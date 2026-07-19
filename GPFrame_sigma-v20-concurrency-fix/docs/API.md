# DGE HTTP API v1 — the isolation boundary

Run: `cargo run -p server --bin dge-serve` (default `127.0.0.1:7464`;
pass an address to override). One JSON request → one JSON response;
`Connection: close`. This document is the whole contract for network
integrations — external features deploy as separate services and talk
to DGE only through these endpoints.

**Security model**: the API is deliberately unauthenticated and binds
loopback by default. Exposing it further is a deployment decision — put
your own transport/auth in front. **Trust model**: the *server's* gate
arbitrates; a `promoted: true` response is a claim about the server's
own 10⁴-sample run under the request's seed. Treat certificates from a
server you don't operate as hearsay: re-gate locally (`POST /v1/gate`)
before relying on them. The verified state itself never crosses the
wire — responses carry sexprs, refusals, certificate data, and emitted
code.

Floats: JSON cannot carry NaN/±Inf, so `/v1/eval` returns non-finite
values as strings (`"NaN"`, `"inf"`) and always includes exact `bits`;
counterexample values are returned as strings for the same reason.

---

## GET /v1/version
→ `{"engine":"deductive-gp-engine","sigma":"v1.7","api":"v1"}`

## GET /v1/alphabet
The Σ op list — enumerate this instead of hardcoding.
→ `{"sigma":"v1.7","ops":[{"name":"+","arity":2}, …],"ext_ops":{"syntax":"(ext:<name> a) | (ext:<name> a b)", …}}`

Note on `ext_ops` (v1.7): `(ext:<name> …)` is legal sexpr syntax for any
arity-1/2 name — it always PARSES. Whether a term using it GATES depends
on server-side registration, which is an **in-process** SDK operation
(`sdk::register_ext_op`, see docs/SDK.md §6) — this HTTP surface does not
currently accept new op registrations over the wire. A `/v1/gate` or
`/v1/certify` call against an unregistered ext op returns
`{"promoted": false, "refused": "…not registered…"}`, not an error.

## POST /v1/extract
Translate one function through one door or all doors.

```json
{"source": "pub fn twice(x: f64) -> f64 { x * 2.0 }",
 "fn_name": "twice",
 "door": "syn"}            // optional; omit to run every door
```
→
```json
{"fn_name": "twice",
 "doors": [
   {"door": "syn", "admitted": true,
    "sexpr": "(* (var 0) 2.0)", "nodes": 3},
   {"door": "ir", "admitted": false,
    "refusal": "…exact reason…", "class": "…trial bucket…"}]}
```
Notes: the `ir` door shells out to `rustc` — give sources a
`#[no_mangle]` if you want stable symbol names; refusal `class` values
are the same buckets `dge trial` histograms use.

## POST /v1/eval
Interpreter-semantics evaluation (the reference semantics).

```json
{"sexpr": "(fold 0.0 (+ acc (elem 0)))",
 "env": [],
 "seqs": [[1.0, 2.0, 3.5]]}
```
→ `{"value": 6.5, "bits": "0x401a000000000000"}`

## POST /v1/gate
Arbitrate candidate vs reference (both sexprs). Optional `seed`
(default fixed): same seed ⇒ same μ′ stream ⇒ reproducible verdict.

```json
{"candidate": "(+ (var 0) (var 0))",
 "reference": "(* (var 0) 2.0)",
 "fn_name": "twice", "seed": 7}
```
→ promoted:
```json
{"promoted": true,
 "certificate": {"n": 10000, "alpha": 0.001,
                 "metric": "bitwise-over-mu-prime", "note": "…"},
 "emitted": "…compilable Rust with certificate comment…"}
```
→ refuted:
```json
{"promoted": false,
 "counterexample": {"minimal_env": [0.0], "minimal_seqs": [],
                    "candidate_val": "1", "reference_val": "0"}}
```
The counterexample is ⊏-minimal and replayable through `/v1/eval`.

→ refused (v1.7 — no verdict could be produced, e.g. an unregistered
extension op; distinct from `refuted`, which is a real bit disagreement):
```json
{"promoted": false, "refused": "extension op `ghost` is not registered -- …"}
```

## POST /v1/certify
The whole front-to-back path: extract via every door, cross-gate all
admissions, emit certified code for the survivor.

```json
{"source": "#[no_mangle]\npub fn twice(x: f64) -> f64 { x * 2.0 }",
 "fn_name": "twice", "seed": 7}
```
→ `{"fn_name": …, "doors": […as /v1/extract…], "report": …as /v1/gate…}`
`report` is `null` when no door admitted (each door's refusal explains
itself).

## Errors
`400` malformed JSON / missing fields / bad sexpr (`{"error": …}`),
`404` unknown endpoint, `413`/`431` size limits (4 MiB body). Handler
errors never 500 with a stack trace — refusals and parse errors are
data, returned in-band.

## Versioning
Path-versioned (`/v1/…`). Within v1: fields are additive-only; the Σ
alphabet is additive-only; refusal *texts* may improve (parse `class`
for stability, not the prose).
