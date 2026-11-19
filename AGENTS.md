# guestfn agent guide

Orientation for AI agents and developers working in this repository. It keeps
what you need to avoid a mistake and points to where the detail lives: the
documentation site under `docs/`, the design records (`docs/one-pager-*.md`),
the comments in the `Makefile`, `.github/renovate.json5`, `deny.toml` and the
workflows, and the code's own comments.

## What guestfn is

guestfn is the project behind two Crossplane composition functions built from
one Rust workspace (the repository is `jonasz-lasut/guestfn`; the packages keep
their names):

- **function-wasm** runs a user-supplied WebAssembly component in a Wasmtime
  sandbox. It's 1.x, under the compatibility promise.
- **function-oci** runs an OCI image in a Firecracker microVM. It's alpha, 0.x
  (`docs/one-pager-oci-runtime.md`, `docs/oci/`).

A module implements the contract of a native function,
`RunFunction(RunFunctionRequest) → RunFunctionResponse`. Users write an
ordinary function, compile it to an ABI v2 component (for Go, `guestfn build`
wraps the `wasip1` module `go build` emits), publish it, usually as an OCI
artifact, and reference it from a Composition step's Input. One installed
function serves any number of modules. The rest of this guide follows
function-wasm unless it says otherwise.

The workspace was ported from an alpha Go implementation in 2026-08. Parity
with Go isn't a goal: the contract is what the conformance goldens and the docs
record.

| deliverable | where | what |
|---|---|---|
| the runtime (host) | `crates/function`, `crates/function-{wasm,oci}` (the binaries, `/function` in each image), `crates/engine*` | the gRPC function: resolves the module the Input names, compiles and caches it, runs it per request; `function validate` runs the same admission offline |
| the guest glue | vendored per guest (Go: `internal/wasmfn` and `internal/bindings`) | linked into a user's `.wasm`: the `run` export, the codec, a logger over the typed `log` import, `GetConfig`, an HTTP client over `wasi:http@0.2`. Not a published module: each guest owns its copy |
| the CLI | `crates/guestfn` | `init` (a guest with its `guestfn.yaml`), `build` (with the runtime's ABI check), `inspect`, `push`, `manifest validate\|show`, `scaffold composition`, `wit update\|fetch` |

## Request flow

`WasmFunction::handle` in `crates/function/src/runner.rs` runs these steps on
the request, function-sdk-rust's `RunFunctionRequest`.
`docs/how-a-request-runs.md` is the reader's version.

1. **Decode and admit.** tonic decodes the request into the SDK's message,
   and one that doesn't decode is a gRPC error. The 0.6 group
   `wasm.fn.crossplane.io` is refused by name (`input::renamed_group`).
   `admission::admit` compiles `compositionPolicy` (content-hash cached,
   malformed Cedar is fatal), checks `limits` against the ceiling flags and
   checks the module's shape.
2. **Resolve `from`.** `from::from_composite` reads `module.from` from the
   observed XR, fenced by the composition layer (`pullModule`,
   `spendCredential`, default-deny). An unset field under `allowEmpty` ends the
   request: the desired state and context are echoed (the SDK's
   `response::to`, `requests_total{outcome="skipped"}`).
   `from::config_from_composite` reads `config.from` the same way.
3. **Registry auth** from the step credential (`oci::auth_for`).
4. **Resolve** (`resolver.rs`, no I/O): the digest comes from the Input (the
   OCI ref, `http.digest`) or from the content hash (`path`). The cosign check
   (`cosign::Verifier`) runs next, before any cache, once per repository and
   digest per process.
5. **Load** (`cache.rs`): memory, then the compiled artifact on disk, then a
   fetch (blob store on disk, verified against the digest) and
   `engine.compile`, which runs the one ABI check.
6. **Manifest and grants.** Read the manifest (an OCI layer, or a
   `guestfn.yaml` a `path`/`http` source names), then
   `admission::admit_requires`: the three-layer AND yields the run's
   capabilities or a fatal result. `manifest.check` holds the configuration to
   the manifest's schema, `sandboxenv` materializes env bindings and checks the
   required credentials, and the runner drops every step credential not
   granted from the request it forwards.
7. **Step slot** (`limits.concurrency`), the wait capped by the gRPC deadline.
8. **Run** (`engine.run`): run slot and memory reservation first, a fresh
   store, WASI argv `["function"]`, no network, `/tmp` and env only as granted,
   the epoch deadline capped by the gRPC deadline, `wasi:http` through the
   egress client. The configuration read at step 2 is written into the
   forwarded Input. The engine takes the request encoded, and its answer is
   decoded: a response that doesn't decode is a fatal result (`cannot decode
   response`).
   - 8a. **Read fence** (`require.rs`, `admission::admit_require`), where a
     layer scopes `requireResource`: every resource selector of the response's
     requirements. A response carrying a fatal result is judged too.
   - 8b. **Output fence** (`compose.rs`, `admission::admit_compose`), where a
     layer scopes `composeResource`: every change to the desired state, diffed
     by composition resource name. A response carrying a fatal result isn't
     judged.
9. **Return** the guest's response, with `meta` added only when the guest
   omitted it. A trap, timeout, OOM, bad ABI, fetch or compile failure is a
   fatal result.

## Layout

```
crates/function/          runtime library (guestfn-runtime), generic over E: Engine; no engine
                          outside dev-dependencies and the testutil feature
  src/cli.rs                serve (default) and validate behind cli::run; every ceiling flag
  src/runner.rs             handle, the steps above; fatal() logs and counts requests_total
  src/grpc.rs               the gRPC transport over the SDK's generated service, mTLS,
                            reflection, gRPC health
  src/grpcmetrics.rs        function-sdk-go's grpc_server_* series as a tower layer
  src/admission.rs          admit, admit_requires, admit_require, admit_compose (validate's too)
  src/authz.rs              the Cedar PDP: CompositionPolicy, OperatorPolicy, the decision memo
  src/require.rs, compose.rs  the read fence's selectors, the output fence's diff (pure)
  src/validate.rs           function validate: exit 0 admitted, 1 refused, 2 tool failure
  src/resolver.rs           Path, HTTP and OCI sources by stated digest; manifests by reference
  src/oci.rs, location.rs   the registry client (testregistry under testutil); reference
                            normalization, canonical_registry
  src/cosign.rs, trustroot.rs  cosign 3 bundles (key and keyless); the trusted root over TUF
  src/cache.rs, store.rs    the memory cache; the content-addressed disk stores and the sweep
  src/egress.rs             HTTP egress: block list, redirects, budgets, audit line
  src/manifest.rs           guestfn.yaml and the Contract enum (wasm@1.0, oci@0.1)
  src/input.rs              the authoritative Input schema (from.rs, sandboxenv.rs, quantity.rs ...)
  src/crd.rs                generates the Input CRD under package/*/input/
  src/logging.rs, ops.rs    --debug and RUST_LOG; /livez, /readyz, /metrics, warm()
  src/testing.rs, testsigstore.rs  test stand-ins (feature testutil)
  tests/                    guests.rs (guest suite), raw_client.rs (unknown fields over the
                            wire), crd.rs
  testdata/                 validate/ (fixture corpus), conformance/ (goldens)
crates/function-wasm/     function-wasm's main; tests/conformance.rs, tests/logging.rs
crates/function-oci/      function-oci's main, with the OCI engine's flags
crates/engine/            the Engine trait, ArtifactStore, Config, RunOptions, RunError (the run
                          metric reads its RunFailure kind, never the words), HttpRequester,
                          slots and memory pool, metrics.rs, RawCodec (the OCI engine's
                          client). No WebAssembly
crates/engine-wasm/       the Wasmtime engine: wasm.rs (compile, ABI check, inspect), run.rs,
                          component.rs (world host, typecheck), wasihttp.rs, componentize.rs
                          (guestfn build's wrap, embed_world), testing.rs (WAT fixtures)
crates/engine-oci/        the OCI engine: Oci over the Sandbox seam (firecracker.rs), rootfs.rs
                          (squashfs), image.rs, run.rs; tests/vm.rs runs real VMs
crates/oci-init/          the guest init, PID 1 in the VM, static musl, Linux only
crates/oci-protocol/      what the init and the host say over vsock
crates/oci-fixture/       the function the VM tests run
crates/guestfn/           the CLI: scaffold.rs, buildcmd.rs, push.rs, inspect.rs, witcmd.rs ...;
                          templates/<lang> (go, rust, ts, python), testdata/<lang> (goldens)
crates/e2e/               the end-to-end suite (feature cluster): src/bin/setup.rs,
                          profiles/<runtime>/<p>/, tests/<runtime>_<p>.rs, guest/ (the committed
                          test module, a workspace of its own)
examples/                 function-wasm-{go,rust,ts,python} share plumbing with their template;
                          function-wasm-{zig,c,cpp,odin,moonbit,csharp} own theirs;
                          function-oci-go; render.sh. Each has a README and a Makefile
wit/guestfn-function.wit  the contract package, guestfn:function@1.0.0
package/{wasm,oci}/       each function's crossplane.yaml, Input CRD and README
docs/, book.toml          the documentation site (mdBook)
build/                    the crossplane build submodule; every make target needs it
Makefile                  e2e.*, oci.*, xpkg.*, the vendoring loops, tools, vale
hack/                     check-generated.sh, design-records.sh, oci-tools.sh, tested-versions.sh
.claude/skills/           cut-release, remediate-cves
crates/function-{wasm,oci}/Dockerfile[.dev]  the release image; the same around prebuilt files
```

## Rules

- **The proxy keeps every field.** The request and the response cross the
  runtime as function-sdk-rust's messages, which since 0.6.0 keep the fields
  they don't know on every message of the function API. The guest receives the
  caller's request without the step credentials it wasn't granted and with the
  Input's `config` resolved, and the response goes back with `meta` added when
  the guest omitted it. Fields newer than the vendored proto survive both ways,
  which `crates/function/tests/raw_client.rs` proves. The byte layout isn't
  kept. Never route the forwarded payload through a message that drops unknown
  fields, as a struct with prost's `Message` derive does.
- **The goldens are the contract.** `crates/function-wasm/tests/conformance.rs`
  runs `function validate` over the fixture corpus and compares stdout, stderr
  and exit codes against `crates/function/testdata/conformance/`. Re-record only
  for a deliberate change, with
  `UPDATE_GOLDENS=1 cargo test -p function-wasm --test conformance`, and say in
  the commit that it changes user-visible behaviour.
- **Refusal wording is contract.** Errors that reach users are `String`s with
  the runtime's exact wording; route a new refusal through the existing
  phrasing patterns. Anything the runtime doesn't carry is refused with a
  message naming it, never silently ignored.
- **Admission and `function validate` stay in lockstep.** Crossplane never
  installs a function's Input CRD, so the runtime enforces every Input rule on
  every request (`admission::admit`, `from::from_composite`,
  `admission::admit_requires`), and `function validate` runs the same functions
  offline with the same ceiling flags. A new rule goes into `admit` or the
  resolver. A new refusal gets a fixture under `testdata/validate/`, a
  conformance golden and an entry in `docs/operators/troubleshooting.md`. A new
  ceiling flag goes into both `serve` and `validate` (`cli.rs`, `validate.rs`).
- **Guest problems are fatal results, never gRPC errors.** A guest's returned
  error becomes `module <desc> failed: run returned an error: <s>` on a fresh
  response, a host panic is caught into a fatal result, and a trap, exit,
  deadline, memory limit, fetch, digest or compile failure is a fatal result
  naming the module. The host never crashes on a guest.
- **Import boundaries.** Only `crates/engine-wasm` imports `wasmtime` and
  `wasmtime-wasi`; `crates/function` names only the `Engine` trait. Only
  `crates/function/src/authz.rs` imports `cedar-policy`.
- **Metrics.** Names, labels and buckets are contract. Never add a module,
  digest or host label: unbounded cardinality.
- **Three-layer authorization.** A capability is granted iff the module's
  manifest requests it, the Input's `compositionPolicy` permits it and the
  operator's `--sandbox-policy-file` permits it. The operator layer is
  default-deny (no policy file, no capability). The composition layer is scoped
  default-permit for sandbox actions and default-deny for fencing `from`
  sources. `composeResource` and `requireResource` are scoped default-permit in
  both layers. The manifest is a request, never a grant.
  `compositionPolicy` and `limits` are read from the Input only, never from the
  XR. `docs/one-pager-three-layer-authz.md`, `docs/operators/cedar-policy.md`.
- **Credentials.** A module receives only the step credentials its admitted
  `requires.env` bindings and `requires.credentials` name, each permitted by
  `spendCredential` in both layers. The pull credential is never forwarded,
  and a manifest naming it is refused.
- **Digests are stated, not discovered.** OCI refs are `@sha256:`-pinned,
  `http.digest` is required, a tag alone is refused, and nothing resolves at
  request time.

## Key concepts

### Input

`guestfn.crossplane.io/v1`, defined in `crates/function/src/input.rs`; the
reference is `docs/operators/input-reference.md`, which you keep in sync with
`input.rs`. The 0.6 group, `wasm.fn.crossplane.io`, is refused by name with no
alias.

- `module`: `type: OCI|HTTP|Path` and exactly one of `oci{ref, credentials}`,
  `http{url, digest, manifestURL, manifestDigest}`, `path` (with
  `manifestPath`), or `from`, the XR field holding that object. With
  `allowEmpty: true`, an unset field skips the step.
- `compositionPolicy`: raw Cedar, the composition author's layer.
- `limits`: `timeout`, `memory`, `concurrency`, each at most its ceiling flag
  (`concurrency` silently capped).
- `config`: `{from, allowEmpty}`, the XR field (under `spec.` or `status.`)
  holding the module's configuration, an object. It's checked against the
  manifest's `config.schema` and written into the forwarded Input, so
  `GetConfig` reads it. Any other key under `config` is refused by name
  (`docs/one-pager-module-config.md`).

There is no `sandbox` field: a capability is a manifest requirement
(`requires.filesystem.privateTmp`, `requires.egress.http`, `requires.env`,
`requires.credentials`) decided by the three layers. The manifest's `composes`
and `requires.resources` are advisory, for the tooling.

### ABI v2

A guest is a WebAssembly component targeting the world
`guestfn:function@1.0.0` (`wit/guestfn-function.wit`): it exports
`run: async func(list<u8>) -> result<list<u8>, string>` (protobuf
`RunFunctionRequest` bytes in, `RunFunctionResponse` bytes out), imports a
typed `log`, and gets WASI 0.3 and 0.2 under the sandbox. A sync-lifted `run`
satisfies it; the `function-sync` world says so for every guest but Rust.

- The ABI check is the world typecheck at load (`engine-wasm/src/component.rs`),
  the one check; `engine.inspect` reports its verdict to `guestfn` and
  `validate --resolve`.
- Egress is `wasi:http` (0.3 `client.send` and 0.2 `outgoing-handler`) bridged
  onto one `HttpRequester` grant (`engine-wasm/src/wasihttp.rs`). A host
  refusal reaches the guest as `internal-error` carrying the refusal string
  (contract), never as a trap.
- ABI v1 (core modules) was removed in 1.0.0. A core module is refused with
  `guestfn_engine_wasm::CORE_MODULE_REFUSAL`; a manifest declaring `abi: 1` is
  refused.
- The engine accepts WebAssembly 2.0 plus extended const and deterministic
  relaxed SIMD, and refuses the other 3.0 proposals. Enabling one is a minor.
- A 1.x minor of the world may only add imports. A change to `run`, to `log` or
  to an existing import's type is `2.0.0`.

`docs/abi-v2.md` is the contract; `docs/one-pager-abi-v2.md` the design.

### Caches

Three stores under `/tmp/guestfn-cache` (`store.rs`), all verified on read:
`modules/<digest>` (fetched blobs), `compiled/<engine version>/<digest>`
(Wasmtime artifacts, each signed under a key drawn per process, so a restart
recompiles) and `manifests/<digest>`. In memory: compiled modules only, idle
TTL 10 minutes, LRU-bounded. `docs/one-pager-cache.md`,
`docs/one-pager-cache-key.md`.

### Readiness and the run bound

gRPC health and `/readyz` start not-ready and flip after warm-up
(`--warm-modules`), which runs while the server already listens; a failed
entry never holds readiness back. `--max-concurrent-runs` is a fair
round-robin scheduler keyed by module digest. Slot, memory-pool and step-slot
waits are capped by the request's gRPC deadline, and a request cut short while
waiting is a fatal result not counted as a run. `docs/operators/readiness.md`,
`docs/one-pager-resource-governance.md`.

### Metrics

`crates/engine/src/metrics.rs` registers the series with prometheus-client,
through the `Labeled{Counter,Gauge,Histogram}` adapters. Counters register
without `_total` and helps without a trailing period: the encoder adds both.
`docs/operators/metrics.md`.

### Signatures

Opt-in through `--cosign-key`, `--cosign-trusted-root` or
`--cosign-trust-instance`, required per repository by the operator policy. A
signature is a cosign 3 Sigstore bundle in an OCI 1.1 referrer of the module's
manifest; legacy `.sig` signatures aren't read. Verification runs before the
caches and never touches the network. `docs/operators/signatures.md`.

### function-oci

A module is an image following `oci@0.1`: its command serves
`FunctionRunnerService` on port 9443 inside a Firecracker VM, as a user that
isn't root. The engine squashes the layers into a read-only squashfs; the init
(`crates/oci-init`) is PID 1. The response returns before the VM is stopped,
and the run keeps its slot and memory reservation until the VMM is reaped
(`crates/engine-oci/src/run.rs`). VM snapshots are alpha, behind
`--enable-vm-snapshots`. function-oci's fatal-result wording isn't pinned by
the goldens. `docs/one-pager-oci-runtime.md`, `docs/one-pager-vm-snapshots.md`,
`docs/oci/`, `docs/contract/compatibility.md`.

## Development guide

### Building

```bash
git submodule update --init            # the build/ submodule; every make target needs it
cargo build --workspace                # the engines, the runtime, both functions, guestfn
cargo run -p function-wasm -- --insecure --module-dir=examples/function-wasm-go
cargo run -p guestfn -- inspect examples/function-wasm-go/fn.wasm
(cd examples/function-wasm-go && GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared -ldflags=-checklinkname=0 -o /dev/null .)
docker buildx build -f crates/function-wasm/Dockerfile --platform linux/amd64,linux/arm64 --target image .
crossplane xpkg build --package-root package/wasm --examples-root examples/function-wasm-go/example --embed-runtime-image=runtime
make oci.init                          # the guest init and the VM tests' fixture, static for musl
```

### Testing

Two tiers (`docs/project/testing.md` has the detail, the profiles, the e2e
module and the workflows): Rust unit and component tests under `cargo test`,
and the Kubernetes end-to-end suite under `make e2e`. Nothing in between: no
render tests, no shell scenarios.

```bash
cargo test --workspace                    # the first tier
cargo test -p guestfn-engine-wasm         # the engine over WAT fixtures and the committed e2e guest
cargo test -p guestfn-engine-oci          # the OCI engine; the VM suite skips without its tools
cargo test -p guestfn-runtime             # runtime units, guest suite, raw client
cargo test -p function-wasm               # the conformance goldens and the log filter
cargo test -p guestfn                     # scaffold goldens, shared plumbing, CLI against an in-memory registry
make oci.test                             # the OCI engine's suite in real VMs (needs /dev/kvm)
make e2e                                  # the second tier: kind, Crossplane, the packages, every profile
make e2e.up && make e2e.wasm-profile.default   # one profile (strict, lifecycle; e2e.oci-profile.<p>); make e2e.down
```

Each example has its own `test` target, and most a `lint` target
(`make -C examples/<name> test`).

Goldens, one variable:

- `UPDATE_GOLDENS=1 cargo test -p function-wasm --test conformance`: the
  conformance goldens (deliberate behaviour changes only).
- `UPDATE_GOLDENS=1 cargo test -p guestfn`: the scaffold goldens after a
  template change.
- `UPDATE_GOLDENS=1 cargo test -p guestfn-runtime --test crd`: the Input CRD
  after a change to `input.rs`.

The guest suite (`crates/function/tests/guests.rs`) builds every scaffold's
golden outside the workspace and runs it through the whole host with the same
expectations: the scaffolds must stay behaviourally identical. It also runs
the unit tests and clippy of `crates/e2e/guest`, a workspace of its own. A
guest whose toolchain is missing is skipped unless
`GUESTFN_REQUIRE_TOOLCHAINS=1`, and the VM suite without KVM unless
`GUESTFN_REQUIRE_KVM=1`. A skip prints a `SKIPPED:` line to stderr and still
reports `ok`.

The e2e test module's `fn.wasm` is committed with `fn.wasm.stamp`, a digest of
its sources and of the module that `crates/engine-wasm/tests/committed_guest.rs`
checks. After editing its source, run `make e2e.guest` and commit both files.

Toolchains: the generators (`protoc`, `nanopb_generator`, `wit-bindgen`) are
pinned in the root `Makefile`, and `make tools` installs them under
`.cache/tools`. Each example's toolchain is in its README and its build job in
`e2e-wasm.yml`.

### Test patterns

Unit tests sit in `#[cfg(test)] mod tests` beside the code, integration suites
under `tests/`. A test takes its stage from the two `testutil` modules, never
from a literal of its own: `guestfn_engine_wasm::testing` for WAT component
fixtures, `guestfn_runtime::testing` for the in-memory registry,
`runtime::builder()`, servers, policies, a TUF repository and the signers
(keyless ones offline, through `testsigstore::Instance`). Expected
`RunFunctionResponse`s are constructed whole and compared with `assert_eq!`.

### Linting

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
git ls-files '*.sh' | xargs shellcheck    # the scripts under hack/ and examples/
actionlint                                # the workflows, each run: block through shellcheck (.github/actionlint.yaml)
make -C examples/<name> lint              # each example's own lint (golangci-lint, zig fmt, moon fmt, ...)
make vale                                 # hack/design-records.sh, then Vale over docs/, README.md, SECURITY.md, package/*/README.md
```

### Coding conventions

- Rust edition 2024. Comments explain why, as the existing ones do; prefer
  self-documenting code.
- English only. Inclusive terms: allowlist/blocklist, primary/replica, main
  branch.
- Conventional commits: `<type>(<scope>): <subject>`, imperative, at most 50
  characters, one logical change per commit (`CONTRIBUTING.md`).
- The Go glue (`internal/wasmfn`) stays buildable natively so a guest's tests
  run natively; only `host_wasip2.go` is `//go:build wasip1`. Edit it in
  `examples/function-wasm-go/internal/wasmfn`, then mirror it to
  `crates/guestfn/templates/go/internal/wasmfn/*.go.tmpl`.

### Documentation site

An mdBook (`book.toml`, `docs/SUMMARY.md`) published from `main` by
`.github/workflows/docs.yml`, which also link-checks it and runs `make vale`.
Build it with `mdbook build` (`cargo install mdbook --locked --version 0.5.4`).

- User-facing behaviour is documented in `docs/`: a new capability, flag,
  field or refusal lands on its page. The READMEs are landing pages only.
- The one-pagers and `docs/abi-v2.md` never move: code and goldens name them
  by path. Any other page that moves gets a redirect in `book.toml`.
- Vale fails on any alert. Spell terms as
  `.vale/styles/config/vocabularies/Guestfn/accept.txt` has them and put
  identifiers in backticks. Passive voice is checked on new pages; `.vale.ini`
  lists the pages exempted after review.

## Common development tasks

### Adding an Input field

1. Add the field to `input.rs` and enforce its rules in `admission::admit`, the
   resolver, or `from.rs` for something the XR may choose. A new capability
   isn't an Input field: it's a manifest requirement (`manifest.rs` `Requires`)
   decided by `admit_requires` under both Cedar layers (a new action in
   `authz.rs`'s schema), then engine `RunOptions` and the sandbox wiring.
2. Add a fixture under `testdata/validate/` and a conformance golden for each
   new refusal; a new ceiling flag goes into `cli.rs` so `serve` and
   `validate` share it.
3. Give the field a doc comment (the CRD's description) and any `schemars`
   attribute, re-record the CRD, and update
   `docs/operators/input-reference.md`.

### Adding a host import

1. Prefer a WASI interface the guest's toolchain already speaks. Link it in
   `component::linker`, reach per-run state through `component::Ctx`, pass what
   it needs in `RunOptions`, keep refusals inside the interface's error type,
   and keep the policy in the runtime behind a trait the engine owns, as
   `HttpRequester` is (`wasihttp.rs` is the worked example).
2. A new world import changes the contract: add it to
   `wit/guestfn-function.wit`, implement it through the bindgen, and carry it
   into every guest (see "Changing the scaffold"). An addition is a 1.x minor;
   a change to an existing type is `2.0.0`.
3. Document it in `docs/abi-v2.md`, cover it with a WAT fixture in the engine's
   tests and, through the guest suite, in every scaffold.

### Changing the scaffold

- Edit `crates/guestfn/templates/<lang>` (`[[ ]]` delimiters; `guestfn.yaml.tmpl`
  is the manifest), then `UPDATE_GOLDENS=1 cargo test -p guestfn`.
- The plumbing (glue, codecs, proto, WIT) goes into the paired example too:
  `examples_share_the_scaffold_plumbing` (`scaffold.rs`, which lists the shared
  paths) holds the go, rust, ts and python examples identical to their
  templates. The zig, c, cpp, odin, moonbit and csharp examples own their
  plumbing, held to the pinned generators by their build jobs.
- The vendored `run_function.proto` copies are one wire contract,
  `crossplane/crossplane`'s `proto/fn/v1/run_function.proto`, each under a
  `Vendored from crossplane/crossplane vX.Y.Z.` header. `make vendor-proto`
  (`PROTO_VERSION=vX.Y.Z` to vendor ahead of Renovate) re-vendors them,
  regenerates the codecs, refreshes the goldens and rebuilds the e2e guest.
- Drift: a build job regenerates with the pinned generator and
  `hack/check-generated.sh` fails on any difference but the version a protobuf
  generator writes in a header. WIT bindings are compared whole.
- A guest commits `wit/world.wit` (a `local:guest` package that includes
  `function` or `function-sync`) and `guestfn.lock`, and pulls `wit/deps/`
  (gitignored) at build: `guestfn build` fetches it first, `guestfn wit fetch`
  without a build. To change the contract package: edit
  `wit/guestfn-function.wit`, publish it (`Publish WIT`), run `guestfn wit
  update` in every template and example, `make gen-bindings`, refresh the
  goldens. `render_matches_the_runtime_world` holds every lock to the root
  package; `wit/guestfn-function.wit` stays in wit-component's printed format.
- `make gen-bindings` regenerates the C bindings (c, zig, odin), the Go
  bindings (mirrored into the go template) and MoonBit's;
  function-wasm-cpp has its own `make gen-bindings`.
  `go.bytecodealliance.org/pkg` moves with the generator
  (`wit-bindgen go --print-remote-pkg-version`).

The `Makefile`'s comments over the vendoring loops have the full procedure.

### Rendering locally

`make -C examples/<name> render` (`examples/render.sh`) is the development
loop, not a test: it builds the runtime, runs `function validate` over every
`example/xr*.yaml`, serves the example with `example/policy.cedar` and
`example/fixtures/`, and prints `crossplane render` of `example/xr.yaml`.
function-oci-go has no render target. `docs/wasm/render-locally.md`.

## Renovate pull requests

Renovate labels its pull requests `e2e:all`. Some need a human or an agent to
act on the branch before they can merge; a generator's pull request never
merges by itself. The rules and reasons are in the comments of
`.github/renovate.json5`.

| the pull request | what to do on its branch |
|---|---|
| the `run_function.proto` version header | `make vendor-proto`, review the diff; the example build jobs fail on codec drift until then |
| protoc or nanopb | nothing if only the header version would move; otherwise `make vendor-proto` |
| wit-bindgen | always `make gen-bindings` (and function-wasm-cpp's own), moving `go.bytecodealliance.org/pkg` with it |
| the docs Cedar highlighting | `make vendor-docs-highlight` |
| the wasmtime group | the `deny` job fails on a second wit-component, wit-parser, wasmparser or wasm-encoder: `cargo update <crate>@<old> --precise <new>` for each, to what `cargo tree -i wit-component` shows wasmtime pulling (`deny.toml`), then fix what the new API breaks in `componentize.rs`. Check a new wasmtime default feature against the explicit list in `crates/engine-wasm/Cargo.toml`. The pull request also moves the `WASI at the version wasmtime-wasi-http X.Y.Z links` comments: move each guest's WASI imports to those versions and run `guestfn wit update` in each |
| `CROSSPLANE_VERSION` to a new minor | shift `CROSSPLANE_PREVIOUS` and `CROSSPLANE_OLDEST` in `release-matrix-e2e.yml` by hand in the same pull request |
| Firecracker | move the guest kernel (`KERNEL_CI`, `KERNEL_VERSION` and checksums) by hand to the kernel that release's CI built; on a new minor, shift `FIRECRACKER_PREVIOUS` and `FIRECRACKER_OLDEST` |
| a sigstore-* minor | pre-1.0: a minor is breaking, so the pull request is a code change |

Pins with no datasource move by hand: Zig, MoonBit, the guest kernel lines,
the `build.zig.zon` dependencies and the `up` CLI (`UP_VERSION`, `UP_SHA256` in
`supplychain.yml`). The scaffold templates' pins and the function package refs
in the render files move at release time (`/cut-release`).

## CI

- The `default branch` ruleset requires a pull request and the checks
  `e2e-gate`, `lint`, `unit-test`, `deny`, `guest-suite` and `vm-suite`.
- `ci.yml`: `lint` (`--all-features`, plus `hack/check-generated-test.sh`,
  `shellcheck` over the scripts and `actionlint` over the workflows),
  `unit-test`, `deny` (`cargo deny` over `Cargo.lock` against `deny.toml`),
  `guest-suite` (`GUESTFN_REQUIRE_TOOLCHAINS=1`) and `vm-suite` (real VMs; on a
  pull request only when the OCI crates, `crates/engine`, the dependencies or
  the pins change). `detect-noop` skips the jobs of a pull request that touches
  documentation alone, by path, never by extension: Markdown or JSON under
  `crates/` is a fixture, a golden or a template.
- `e2e.yml` runs on a pull request's own events, and its labels select the
  scenarios; several labels run their union, and a labelled pull request runs
  again on every push. `e2e-gate` reports the verdict.

| label | runs |
|---|---|
| `e2e:all` | every scenario |
| `e2e:wasm:all`, `e2e:oci:all` | every scenario of one runtime |
| `e2e:wasm:default`, `e2e:wasm:strict`, `e2e:wasm:lifecycle` | one `wasm` profile |
| `e2e:oci:default`, `e2e:oci:privileges`, `e2e:oci:snapshots` | one `oci` profile |
| `e2e:wasm:examples` | the WebAssembly examples' `build (<lang>)` jobs, with drift checks |
| `e2e:oci:examples` | `build (oci-go)` |

- `e2e-caches.yml` saves the caches on `main`; a pull request restores and
  saves none.
- `release-matrix-e2e.yml` runs on every push to a `release-function-wasm-*`
  or `release-function-oci-*` branch: the function's profiles on three
  Crossplane minors and, for function-oci, the VM suite on nine Firecracker ×
  kernel pairs (only the pinned pair blocks). Its `tested-versions` artifact
  feeds `README.md`'s tables (`hack/tested-versions.sh`).
- `.github/CODEOWNERS` makes the owner the code owner of `.github/workflows/`:
  a pull request runs its own copy of the workflows.
- Every job has a `timeout-minutes` of about three times its usual run, never
  under ten.

## Releasing

Each component has its own versions: tags and GitHub releases
`function-wasm-vX.Y.Z`, `function-oci-vX.Y.Z`, `guestfn-vX.Y.Z` and
`wit-vX.Y.Z`, release branches `release-<component>-X.Y`. An OCI artifact's tag
is the version alone (`function-wasm:vX.Y.Z`; the WIT world's the bare
`X.Y.Z`). The `release tags` ruleset refuses moving or deleting a tag, so a
wrong tag is corrected by a new version; `release branches` refuses deleting or
force-pushing a `release-*` branch, and every release branch stays.

Two skills drive releases; use them rather than improvising the sequence:

- `/cut-release` (`.claude/skills/cut-release/SKILL.md`): a minor or major of
  one component from `main` HEAD, through the release matrix, the tag, the
  release and `Publish Function`. The bump size is the user's choice, never
  inferred.
- `/remediate-cves` (`.claude/skills/remediate-cves/SKILL.md`): a patch release
  on a release branch for findings from `grype-scan.yml`.

`publish-function.yml`'s `guard` job refuses a ref other than the release's
tag, a version already published and a commit no release matrix passed; its
`without-matrix` and `overwrite` inputs lift the last two on the owner's word
only. It signs through `supplychain.yml@main`, never `./`, so the keyless
signer identity users pin stays the same on every release line. A release
branch runs the workflow files it was cut with.
`Publish guestfn`, `Publish WIT` (refuses a version already published) and
`Publish Examples` (publishes an example whose `guestfn.yaml` version is new)
run by hand. `SECURITY.md` is the dependency CVE policy.

## Design records

The one-pagers under `docs/one-pager-*.md` follow one pattern: an H1 naming
the feature in sentence case, then `* Owner: @jonasz-lasut (Jonasz Małecki)`,
`* Reviewers: function-wasm maintainers` and
`* Status: draft | implemented, revision x.y | withdrawn | superseded by …`,
the values lowercase. Bump the revision when the design changes and move the
row in `docs/project/design-records.md` and the entry in `docs/SUMMARY.md` with
it; `hack/design-records.sh` (the first step of `make vale`) checks both. A
record describes the design as it is, with discarded options under
Alternatives and each decision carrying its decider and date.

## Design decisions

One line each; the record named holds the reasoning.

- Rust host for ABI v2 on the component model (Jonasz, 2026-08-25):
  `docs/one-pager-abi-v2.md`.
- The proxy forwards function-sdk-rust's messages (Jonasz, 2026-11-19): since
  0.6.0 they keep unknown fields on every message. It was wire-level from
  2026-08-25, because a prost round-trip silently dropped the fields of a guest
  built on a newer proto than the runtime. A module sees only the step
  credentials it was granted (2026-10).
- The gRPC server is assembled in `grpc.rs`, not by the SDK's `Server`
  (Jonasz, 2026-08-26): it has one caller, and owning it lets warm-up flip
  gRPC health while requests are already served.
- Wasmtime is the only reader of a WebAssembly module (Jonasz, 2026-08-17):
  no second decoder, so every verdict `guestfn` or `validate` prints is the
  runtime's.
- A `FROM scratch` image holds the module at `/fn.wasm`, and the resolver
  never picks "the first `.wasm` file"; raw `application/wasm` layers are
  recommended (Jonasz, 2026-08-17).
- The manifest is a layer of the OCI artifact, not a custom section, and a
  request, never a grant (Jonasz, 2026-08-17):
  `docs/one-pager-module-manifest.md`.
- Three-layer authorization with the operator's Cedar file as the only
  enabler, default-deny (Jonasz, 2026-08-19); the output fence scoped
  default-permit so an upgrade refuses no module's output (Jonasz,
  2026-10-27): `docs/one-pager-three-layer-authz.md`,
  `docs/one-pager-read-fence.md`.
- Sandbox filesystem and env are WASI, not ABI; no host mounts; egress goes
  through the host, never a socket (Jonasz, 2026-08-16):
  `docs/one-pager-sandbox.md`, `docs/operators/http-egress.md`.
- A fresh instance per request; disk caches bounded by an LRU sweep; compiled
  artifacts keyed per process, with no flag (Jonasz, 2026-10-28):
  `docs/one-pager-cache.md`, `docs/one-pager-cache-key.md`.
- No shared guest SDK module: every guest owns its glue (Jonasz, 2026-08-19).
- Go and MoonBit guests reach ABI v2 through `guestfn build`'s embed and wrap,
  not componentize-go (Jonasz, 2026-10-07).
- C guests build with `zig cc` and nanopb over wit-bindgen's C bindings; Zig
  reads the same bindings through translate-c (Jonasz, 2026-08-19 and
  2026-10-07). WASI argv is always `["function"]`: an empty argv traps at
  `_initialize`.
- `guestfn init` has a template only for a language with an official
  `function-sdk-<lang>`: go, rust, ts, python (Jonasz, 2026-11-08).
- Warm-up runs while the server listens; the run bound is the engine's, fair
  round-robin per module digest: `docs/one-pager-resource-governance.md`.
- `function validate` lives in the runtime binary, so it uses the operator's
  flags, policy and version.
- No local-loop machinery: `guestfn build`, `crossplane render` and
  `function validate` (Jonasz, 2026-08-17).
- The project is guestfn, the packages keep their names (Jonasz, 2026-11-08):
  crates, metric series, cache directory and test switches moved at 0.6 with
  no alias; `function-wasm` and `function-oci` names stay.
- The contract package is `guestfn:function@1.0.0` with a sync world beside
  the async one; `guestfn wit update` bumps and the build pulls what
  `guestfn.lock` pins (Jonasz, 2026-11-10): `docs/abi-v2.md`.
- A crate per engine and per function (Jonasz, 2026-11-09):
  `docs/one-pager-oci-runtime.md`.

## Key dependencies

- `wasmtime`, `wasmtime-wasi`: `crates/engine-wasm` only; `engine::version()`
  namespaces the compiled cache on a bump. The default features are listed
  explicitly less `cache` and `threads`.
- `wit-component`, `wit-parser`, `wasmparser`, `wasm-encoder` (through `wat`):
  held in `Cargo.lock` at the versions wasmtime pulls, a second copy refused by
  `deny.toml`. `wasi-preview1-component-adapter-provider` moves with the
  wasmtime group.
- `backhand` (zstd): function-oci's graph only. `cargo tree -p function-wasm -i
  zstd-sys` and the same for `guestfn` must find nothing.
- `function-sdk-rust`: the messages, which keep unknown fields, the generated
  service and the helpers.
- `cedar-policy`: both policy layers, `authz.rs` only.
- `sigstore-verify`, `sigstore-trust-root`, `sigstore-types`, `sigstore-tuf`:
  keyless verification and TUF. The `sigstore` crate isn't used.
- `reqwest` (blocking, rustls): the egress and registry clients.

## Where the detail lives

- `docs/operators/`: the Input reference, runtime flags, the Cedar policy
  reference (schema, ten actions, refusal sentences), trust model,
  signatures, metrics, readiness, troubleshooting.
- `docs/contract/compatibility.md`: the 1.x promise the goldens enforce.
- `docs/abi-v2.md`: the host/guest contract.
- `docs/project/testing.md`: the two tiers, where a new test goes, the e2e
  profiles and the workflows.
- `docs/project/development.md`: the commands CI runs.
- `docs/project/design-records.md`: the index of the one-pagers.
- `docs/one-pager-use-case-examples.md`, each example's README: the examples.
- `.github/renovate.json5`, `deny.toml`, the `Makefile`: why each pin and
  vendoring loop is the way it is.
- `.claude/skills/cut-release/SKILL.md`, `.claude/skills/remediate-cves/SKILL.md`:
  releasing.
