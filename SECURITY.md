# Security Policy

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Use one of these private channels instead:

1. **GitHub Security Advisories** (preferred). Go to the repository's
   [Security tab](https://github.com/ChristopherDavitt/RAGgy/security) and
   click **Report a vulnerability**. This creates a private advisory that
   only the maintainers can see.
2. **Email**: **<TODO: fill in — e.g. security@yourdomain.tld>**. PGP key
   available on request.

Please include:

- A description of the issue and its potential impact.
- Steps to reproduce, or a minimal proof-of-concept.
- The version or commit hash of RAGgy you tested against.
- Whether the issue is already public anywhere.

We aim to acknowledge reports within **72 hours** and to ship a fix or a
mitigation within **30 days** for confirmed issues, faster when the severity
warrants it.

## Scope

RAGgy is a **single-user, local-first tool**. The default threat model
assumes:

- The machine running RAGgy is trusted.
- The HTTP server (`raggy serve`) is bound to `localhost` and is not exposed
  to the public internet. It is **unauthenticated by design**.
- Ingested content is supplied by the operator — RAGgy does not sandbox
  untrusted documents beyond what the underlying parsers (e.g. `pdf-extract`,
  `pulldown-cmark`) provide.

**In scope** for security reports:

- Remote code execution, memory-safety bugs, or panics reachable from
  untrusted input (e.g. a malformed PDF that crashes or corrupts state).
- Path traversal, arbitrary file read/write, or other sandbox escapes from
  ingestion.
- SQL injection or similar in the query or HTTP paths.
- Issues in the MCP surface (`raggy mcp`) that a malicious MCP client could
  exploit.
- Secret material (API keys, tokens) being written to logs or telemetry
  unintentionally.

**Out of scope**:

- Exposing the HTTP server on the public internet without a proper auth
  proxy. Don't do this; it's documented as unsupported.
- Denial-of-service from feeding RAGgy an unbounded corpus (by design the
  tool trusts the operator).
- Supply-chain reports for transitive dependencies — please report those
  upstream. We will respond to advisories that materially affect RAGgy.

## Disclosure

Once a fix is released, we will publish a GitHub Security Advisory crediting
the reporter (unless they prefer to remain anonymous) and referencing the
patched version.

## Questions

For non-urgent security questions that aren't a disclosure, open a discussion
on GitHub or email the address above.
