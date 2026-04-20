# Contributing to RAGgy

Thank you for considering a contribution. RAGgy is maintained by a small team
and we want every PR to be a good use of your time — please read this page
before opening one.

## Ground rules

- Be kind. See the [Code of Conduct](./CODE_OF_CONDUCT.md).
- Small, focused PRs beat big ones. One logical change per PR.
- If you're about to spend more than an afternoon on something, open an issue
  first so we can agree on the shape before you write the code.

## Good first issues

The best places to start:

- Issues tagged **`good first issue`** or **`help wanted`**.
- Extending the query parser (`src/query/parser.rs`) — new operators, new
  date phrases.
- New file-type extractors in `src/extractors/` and `src/ingest/`.
- Documentation: if something in the README, `ARCHITECTURE.md`, or CLI help
  confused you, fix it.
- Tests — especially fixture-style integration tests in `tests/` for parts of
  the pipeline that don't yet have coverage.

If you want to tackle something larger (a new index backend, auth, a new
surface), open an issue titled `proposal: …` first.

## Development setup

```bash
# Rust 1.75+
git clone https://github.com/ChristopherDavitt/RAGgy
cd RAGgy
cargo build

# Run the tests (fast; no ONNX models required)
cargo test

# Run the CLI against a scratch database
RAGGY_HOME=/tmp/raggy-dev cargo run -- init
RAGGY_HOME=/tmp/raggy-dev cargo run -- index ~/some-folder
RAGGY_HOME=/tmp/raggy-dev cargo run -- query "your query"
```

If you're working on anything touching embeddings or NER, you'll need
`raggy init` to have run at least once to fetch the models.

## Before you open a PR

Run these locally. CI will run them too; saving yourself a round-trip is
polite.

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

If your change affects behavior that a user would notice, update:

- The relevant section of `README.md`, if the feature is user-facing.
- `ARCHITECTURE.md`, if you changed how the pipeline works.
- Tests. New behavior needs a test; regressions need a regression test.

Documentation-only or test-only PRs don't need to update every section. Use
judgment.

## Commit and PR style

- **Commit messages**: imperative mood, capitalized, no trailing period. Good:
  `Add YAML frontmatter escaping`. Bad: `added escaping.`
- Keep commits logically coherent. If you notice a small unrelated fix while
  working, put it in a separate commit.
- **PR title**: matches the commit style. Short and direct.
- **PR description**: what the change does, why, and how you verified it
  works. Link any related issues.

## Developer Certificate of Origin

We use the **[Developer Certificate of Origin](https://developercertificate.org/)**
(DCO) instead of a Contributor License Agreement. By signing off on your
commits, you're asserting that you wrote the code (or have the right to submit
it) and that the project can use it under the MIT License.

Sign off each commit with `git commit -s`, which appends a line to your commit
message:

```
Signed-off-by: Your Name <you@example.com>
```

Your signoff name and email must match the `user.name` and `user.email` in
your git config. If you forgot to sign off, amend or rebase to add it:

```bash
# For the last commit
git commit --amend -s --no-edit

# For an in-progress branch
git rebase --signoff main
```

CI will block PRs that contain unsigned commits.

The full DCO text is at
[developercertificate.org](https://developercertificate.org/) and reproduced
below for convenience:

```
Developer Certificate of Origin
Version 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off)
    is maintained indefinitely and may be redistributed consistent
    with this project or the open source license(s) involved.
```

## Reporting bugs

Open a GitHub issue with:

- What you did (the exact command, if a CLI bug).
- What you expected to happen.
- What actually happened (including relevant stderr output).
- The output of `raggy status`.
- Your OS, Rust version (`rustc --version`), and RAGgy version or commit.

For security-sensitive bugs, **do not open a public issue** — see
[SECURITY.md](./SECURITY.md).

## Asking questions

Use GitHub Discussions for questions, ideas, and conversations that don't fit
the issue tracker. The issue tracker is for reproducible bugs and concrete
proposals.

## License

By contributing, you agree that your contributions will be licensed under the
MIT License that covers the project. See [LICENSE](./LICENSE).
