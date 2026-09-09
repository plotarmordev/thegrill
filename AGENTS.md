# Development guide

Read README.md, CONTRIBUTING.md, and docs/PROJECT.md before changing behavior.

- Keep The Grill a standalone Rust CLI. Reuse existing modules and avoid speculative abstractions or dependencies.
- Preserve protocol compatibility, deterministic identities, and fixed example bytes. Grading behavior changes require an explicit implementation revision.
- Treat task packs, model outputs, and saved evidence as untrusted input. Never turn data files into executable instructions.
- Do not hide missing answers, failed attempts, or uncertainty to improve a score.
- Verify changes with observable behavior. Tests must use synthetic inputs and local fixtures, not live model endpoints.
- Keep credentials, sensitive datasets, and generated results out of version control.
- Use feature branches and pull requests. Publishing, rewriting shared history, releases, and infrastructure changes require explicit maintainer authorization.
- Report only checks actually performed and distinguish implemented behavior from proposed work.
