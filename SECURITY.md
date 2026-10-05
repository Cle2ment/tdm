# Security Policy

## Supported versions

TDM is pre-1.0. Only the latest commit on `main` receives fixes and is
supported.

| Version | Supported |
| --- | --- |
| `main` | ✅ |
| release tags (`v0.1.2`, …) | ❌ — snapshots; fixes land on `main` and ship in the next release |

## Reporting a vulnerability

Please **do not** open a public issue for security reports. Use GitHub's private
vulnerability reporting (Security → Advisories → "Report a vulnerability"), or
contact the owner directly via GitHub ([@Cle2ment](https://github.com/Cle2ment)).

Please include: affected component (crate/package and version or commit), a
reproduction or proof of concept, and the impact you see. Expect an
acknowledgement within a few days; we will coordinate disclosure with you.

## Scope notes (what "security" means here)

TDM handles API keys and records judgment requests. The areas that matter most:

- **Key material.** Provider API keys live in `auth.toml` / `TDM_*_API_KEY`
  environment variables and are read only by the backend (`tdm-runtime` /
  `tdmm` / the napi binding). They must never appear in logs, audit rows,
  error messages, `Debug` output, or adapter processes. Redaction is enforced
  by manual `Debug` impls and CLI output filters — if you find a leak path,
  that is a security report.
- **The audit database.** `audit.db` stores the full request JSON of every
  judgment, including the `state` payload. Adapters are responsible for not
  placing secrets into `state`; treat the database as sensitive at rest.
- **Error detail.** Provider error messages include truncated response bodies
  (bounded to ~500 chars). If an upstream provider can echo sensitive input
  back in an error body, that truncation boundary matters.

## Release integrity

npm publishing uses **OIDC trusted publishing** from GitHub Actions
(`.github/workflows/release.yml`) with automatic SLSA provenance — no
long-lived npm tokens or publish secrets are stored in the repository or CI.
The first-ever publish of each package was seeded manually so trusted
publishing could attach; all later releases are automated.

Dependency vulnerabilities are tracked via CI; report anything you believe we
have missed through the same channels above.
