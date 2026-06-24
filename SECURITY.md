# Security Policy

## Supported versions

Coppa is at an early (0.x) stage. Security fixes are applied to the latest
`main`; there are no separately maintained release branches yet.

| Version          | Supported |
|------------------|-----------|
| `main` (latest)  | ✅        |
| older commits    | ❌        |

## Reporting a vulnerability

**Please do not report security issues in public GitHub issues.**

Report vulnerabilities privately using GitHub's **"Report a vulnerability"**
button under this repository's **Security** tab (Security → Advisories). This
keeps the report private until a fix is available.

> Maintainer note: this requires *Private vulnerability reporting* to be enabled
> in **Settings → Code security and analysis**. If the button is not visible,
> open a minimal public issue asking for a private contact channel — **without
> any details** — and we'll follow up.

## Scope

Coppa is a **reference implementation**, not a hardened production modem. The
most security-relevant surfaces are:

- The host servers in `coppa-host` (VARA-style TCP, WebSocket, KISS TNC). These
  default to binding `127.0.0.1`; exposing them on a network is at the operator's
  risk, and they have **no authentication**.
- The C FFI in `coppa-ffi`.
- The frame/decode path, which parses untrusted over-the-air data.

Reports against these surfaces are especially welcome.

## What to expect

This is a small, volunteer-maintained project, so response is best-effort. We
will acknowledge valid reports, work on a fix, and credit you in the disclosure
unless you prefer otherwise.
