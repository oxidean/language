# Security policy

oxl is a programming-language toolchain: a compiler that emits C, a small
runtime, a standard library, and a module system that fetches source over
git. Reports are taken seriously.

## Supported versions

Security fixes land on `main` and the latest release tag. Older tags are
not patched, so please upgrade before reporting a bug against an old
version.

## Reporting a vulnerability

Do not file public issues, pull requests, or discussions for
vulnerabilities.

Use [GitHub private vulnerability reporting](https://github.com/oxidean/oxl/security/advisories/new)
on `oxidean/oxl`.

A good report includes:

- the release tag or commit you tested
- your OS and toolchain (`cc --version`, and the Rust toolchain if you
  built the seed compiler yourself)
- steps to reproduce and the impact you see — ideally a minimal `.oxl`
  program or module layout that triggers it
- whether the issue needs a specific configuration, module source, or
  permissions to trigger

Please give us reasonable time to investigate and release a fix before
disclosing publicly. We will acknowledge reports as soon as we can; this
is an early-stage project, so there is no formal SLA.

## Scope

In scope: the oxl toolchain and everything it ships — the compiler front
end (lexer, parser, checker, diagnostics), the C code generator, the
runtime, the standard library, the LSP server, and the module system
(`oxl get`, the module cache, `oxl.sum` verification). Module supply-chain
issues — checksum bypasses, cache poisoning, malicious manifest handling,
unsafe git invocation — are squarely in scope.

Out of scope:

- bugs in programs *written in* oxl (report those to their authors)
- weaknesses of a specific deployment or host (open ports, unpatched
  system compiler or libraries)
- issues in third-party dependencies without a demonstrated exploit path
  through the toolchain
- denial-of-service via resource exhaustion that applies to any compiler
  given pathological input

When in doubt, report privately anyway and let us triage.
