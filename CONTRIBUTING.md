# Contributing to Echo

Use the [development guide](docs/development.md) for prerequisites, pinned toolchains, platform build recipes and the six required checks. Follow [README's quick start](README.md#quick-start) to understand the user flow, and [architecture](docs/architecture.md) to locate the relevant module. Report vulnerabilities through [SECURITY](SECURITY.md#report-a-vulnerability).

## Change conventions

Keep changes focused on an observable behavior or a documented problem. Preserve unrelated work. Explain the trigger, resulting behavior, affected platforms, checks run and remaining evidence limits in the pull request. Include a regression test for a behavior fix. Do not claim a physical or packaged path from a simulated test.

Keep logic with its feature. Svelte components compose views; feature-local controllers own interaction logic that tests can import directly. Reuse existing native clients and revision-aware projections. Rust owns capture, credentials, provider requests, persistence and native delivery. Preserve native window authorization, bounded resource ownership, cancellation and stale-event rejection when changing those paths.

Use plain concrete prose and one term per concept. Cite source paths when documenting mechanisms. Keep each fact with its documentation owner: README for product use, provider catalog and privacy; architecture for implementation; development for build commands; SECURITY for reporting and accepted risks; this file for contribution conventions; the [brand guide](design/brand/README.md) for artwork.

Never commit API keys, tokens, cookies or private keys. Replace secret values in reports and logs with `<redacted>`. Enable the pre-push secret scan once per clone:

```sh
git config core.hooksPath .githooks
```

The [.githooks/pre-push](.githooks/pre-push) hook requires Gitleaks on PATH and blocks a push when scanning fails. On macOS it can be installed with `brew install gitleaks`. CI also scans secrets as described in [development](docs/development.md#ci).

## Tests and evidence

Test the smallest public behavior that can catch the defect. Demonstrate that the regression test fails before the fix. Prefer intact components and real local integrations over assertions about private implementation details.

- Frontend controller tests import feature-local controllers directly. [render-svelte.ts](tests/helpers/render-svelte.ts) compiles intact components, and [mount-svelte.ts](tests/helpers/mount-svelte.ts) provides a per-file happy-dom harness with cleanup. Preserve real component composition; avoid extracting fragments solely to make tests pass.
- Native capture tests use fake hardware to exercise the coordinator, callback blocks, segmenting and artifact leases. They do not acquire a physical microphone.
- Storage tests use temporary SQLite databases to exercise revisions, validation, retention and deletion cleanup. They do not establish removal from backups or older free pages.
- Provider tests use controlled responses and real local TCP listeners to inspect request bodies, response bounds and HTTP policy. Loopback proves the adapter path, not a real provider account. Report sandbox port-binding failures explicitly.
- Segment tests live in [commands/stt/segments_tests/](src-tauri/src/commands/stt/segments_tests/). The [architecture test map](docs/architecture.md#tests-and-evidence) identifies its modules and the layers each loop exercises.

Run the [six required checks](docs/development.md#required-checks). Add the exact failed or skipped commands and reasons to the pull request. For capture, shortcut, permission or delivery changes, record [physical acceptance](docs/development.md#physical-acceptance) on each affected platform. Controller and component tests do not establish rendered geometry, real keyboard behavior, accessibility audits, OS permission grants or clipboard-manager exclusions.
