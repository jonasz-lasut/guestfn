# Contributing to guestfn

This guide is for anyone opening an issue or a pull request against guestfn,
the repository behind function-wasm, function-oci and the `guestfn` CLI. The
[Development](https://jonasz-lasut.github.io/guestfn/project/development.html)
and [Testing](https://jonasz-lasut.github.io/guestfn/project/testing.html)
pages of the manual have the detail it links to.

## Before you start

Open an issue before the pull request for anything beyond a small fix: a new
capability, flag, Input field or refusal, a change to a guest contract, or a
design change. Start the title with the component, as in
`function-oci: the init's exit report can be missing from the console log`.
The components are `function-wasm`, `function-oci`, and `guestfn` for the CLI
and anything across both runtimes. A maintainer reads each new issue and
labels its component and kind.

Report a vulnerability through the private form described in
[SECURITY.md](SECURITY.md), never in a public issue.

## Build and test

```shell
git submodule update --init   # the build/ submodule every make target needs
cargo build --workspace
```

Unit and component tests run under `cargo test --workspace`. The end-to-end
suite installs the packages in a kind cluster with `make e2e`, which needs
Docker. [Testing](https://jonasz-lasut.github.io/guestfn/project/testing.html)
says which of the two tiers a new test belongs in.

Run these before you open a pull request:

```shell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check advisories bans licenses sources   # after a dependency change
make vale                                           # after a change to docs/, a README or SECURITY.md
```

The guest suite in `cargo test --workspace` skips a guest whose toolchain
isn't installed and still reports `ok`. Each skip prints a `SKIPPED:` line.
`GUESTFN_REQUIRE_TOOLCHAINS=1` turns a skip into a failure, as CI does. An
example under `examples/` has its own `test` target in its Makefile.

## Commits

Commits follow the conventional format, `<type>(<scope>): <subject>`, where
the type is one of `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`
or `perf`, and the scope is optional: `fix(quantity): refuse a fraction that
overflows`. The subject is imperative, at most 50 characters, with no period.

Keep one logical change per commit. Write the subject alone for a small
change. For a larger one, add a body wrapped at 72 columns that says what
changed and why, and end it with `Fixes #N` where it closes an issue. Branch
names use the same types: `fix/source-checks`,
`docs/promise-and-small-corrections`.

## Pull requests

The title follows the commit format. The description is a few sentences of
plain prose: what changes, why, and whether a refusal's wording, a flag or a
conformance golden changes.

The `main` branch accepts a pull request once these checks pass:

| check | what it runs |
|---|---|
| `lint` | `cargo fmt`, `cargo clippy` with every feature, the drift check's own test |
| `unit-test` | `cargo test --workspace`, without the guest builds |
| `deny` | `cargo deny` over `Cargo.lock` |
| `guest-suite` | every scaffold built and run through the host, with every toolchain required |
| `vm-suite` | the OCI engine's suite in real VMs, on a pull request that touches the OCI engine or its pins |
| `e2e-gate` | the end-to-end scenarios selected by labels |

A pull request that only touches documentation skips the code checks. Labels
select the end-to-end scenarios: `e2e:all` runs every one,
`e2e:wasm:<profile>` and `e2e:oci:<profile>` run one profile, and
`e2e:wasm:examples` and `e2e:oci:examples` build the examples. A pull request
with no `e2e:` label runs none. Adding a label takes triage access, which
maintainers have.
[The workflows](https://jonasz-lasut.github.io/guestfn/project/testing.html#the-workflows)
lists every label.

The conformance goldens under `crates/function/testdata/conformance/` record
the runtime's refusals and their exact wording, which operators match in logs.
A change to a golden is a user-visible behaviour change. Re-record it with
`UPDATE_GOLDENS=1 cargo test -p function-wasm --test conformance` only when
you mean to change the behaviour, and say so in the commit and the pull
request.

Document user-facing behaviour on its page under `docs/`; the READMEs only
link there. A new Input field goes into the
[Input reference](docs/operators/input-reference.md), and a new refusal needs
a fixture under `crates/function/testdata/validate/`, a conformance golden and
an entry in [Troubleshooting](docs/operators/troubleshooting.md).

The owner reads every change under `.github/workflows/` before it merges
(`.github/CODEOWNERS`), since a pull request runs its own copy of the
workflows.

## Compatibility

From 1.0.0, function-wasm keeps the Input's fields, the runtime flags and
their defaults, the wording of refusals, the module manifest, the ABI v2
world, the metric names, `function validate`'s exit codes and the `guestfn`
commands stable across 1.x. A minor release may add or widen. A removal,
rename or narrowing waits for a major release, after a deprecation that runs
at least one minor. In a patch, only a security fix may narrow what's
accepted. function-oci is alpha, and a 0.x release may change anything it has.
[Compatibility](docs/contract/compatibility.md) has the full list.

## Design records

Write a design record for a design change, a one-pager under
`docs/one-pager-<name>.md`. It opens with its owner, reviewers and status
line, and it describes the current design, with the discarded options under
Alternatives. A change to the design bumps the record's revision and its row
in [Design records](docs/project/design-records.md). `make vale` fails until
that row and `docs/SUMMARY.md` match the record.

## Language and style

- English only, in code, comments, docs, commits and configs.
- Inclusive terms: allowlist and blocklist, primary and replica, main branch.
- Self-documenting code over comments. A comment explains why, not what.
- In prose, a short dash or a colon, never an em dash. Vale enforces this and
  the project vocabulary in the documentation.

## Agents

If you're an AI agent, read [AGENTS.md](AGENTS.md) before you change
anything. It has the layout, the rules every change must keep and what each
kind of Renovate pull request needs before it can merge.

Commits and pull request descriptions contain no AI attribution: no
`Co-Authored-By` or `Claude-Session` trailer and no generated-with footer.
In the history, only a few dozen commits and six pull request descriptions,
dated 2026-11-05 to 2026-11-08, contain them.
