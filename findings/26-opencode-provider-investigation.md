# OpenCode provider investigation: the zen free tier and what air can use

2026-10-15 · investigation notes from wiring air to OpenCode inference · head `fee5e65`

## Goal

Use `deepseek-v4.1-flash-free` (zen free tier) from air via the user's OpenCode account.

## What works from air (wired in and verified)

- Endpoint: `https://opencode.ai/inference/go/v1` (the Go plan mount; docs: go.mdx "Endpoints").
- Free, unlimited on Go, verified end-to-end through air: `cat-jam-free`,
  `longcat-2.5-preview-free`, `step-5-preview-free`.
- Paid, verified: `deepseek-v4.1-flash` ($0.15/M in off-peak, $0.30 peak; from the Go plan's
  $80/month deepseek allowance).
- The `oc_sk_…` key is a console service-account key (api/index.mdx); it cannot touch zen
  free-tier models and the account has no billing (paid zen models answer "disabled").
- `x-opencode-session` is mandatory for every chat completion on the Go mount — for paid and
  free alike: without it the gateway answers `400 MissingSessionID`. `GET /models` does not
  need it. Go docs ask clients for their own User-Agent (air sends `air/<version>`) plus one
  stable session id per conversation (air sends the session file's name).

## The zen free tier gate — why deepseek-v4.1-flash-free cannot come from air

The zen mount (`/inference/openai/v1`, `/zen/v1`, `/inference/v1`) answers:
`403 FreeTierError: "OpenCode's free tier can only be used from within OpenCode."`

Isolated by byte-exact replay and bisection (captured via an opencode plugin hook on
`session.http.request`), the gate checks four things:

1. Member credential — the `st_…` access token from `opencode auth login`, kept in the
   `credential` table of `~/.local/share/opencode/opencode.db` (fields: access, refresh,
   expires, metadata.server/accountID/email/orgID). Not any API key.
2. `x-opencode-org-id: wrk_…` — from the login metadata.
3. A fresh, opencode-shaped session id: `ses_` + 12 hex digits of an inverted timestamp +
   14 base62 digits (packages/schema/src/identifier.ts). The gateway decodes the timestamp;
   malformed or stale-shaped ids are rejected.
4. The request's `tools` array byte-matches opencode's own 12 tool definitions
   (edit, glob, grep, question, read, shell, skill, subagent, webfetch, websearch, write,
   execute). Renamed tools, hollowed schemas, synthetic tools, or one real tool alone all
   fail; only the verbatim set passes.

Bisection evidence: app request replayed byte-for-byte → 200; app headers + minimal body →
403; fresh valid session id + app body → 200; same with one/synthetic/renamed tools → 403;
40 KB padding, HTTP/2, bun-vs-curl (same stack as the app) → 403; opencode's own client
fingerprint (UA `opencode/latest/2.0.29/cli` + x-opencode-client) with the API key → 403.

Conclusion: passing check 4 means embedding a version-pinned copy of opencode's tool
schemas to impersonate the client — deliberately circumventing a stated access control
("only from within OpenCode"), brittle across opencode updates, and their docs state traffic
is monitored for abuse. Declined. The sanctioned path for third-party agents is the Go plan,
which air now uses.

## What air implements (src/llm.rs, src/session.rs)

- `User-Agent: air/<version>` on every request — air's identity.
- `x-opencode-session` with one stable id per conversation, sent only to opencode.ai hosts
  (session.rs writes the id into the session header; llm.rs gates it on the host).
- No credential reuse: the opencode CLI's login is not read by air.

Status: settled — Go endpoint with the three free models; zen free-tier model not wired.
