# Security

## Reporting a vulnerability

Use GitHub's private vulnerability reporting option when available. Otherwise, open an issue requesting a secure contact channel without including exploit details, credentials, or sensitive attachments.

Include the affected version, a concise description, and a minimal reproduction using synthetic data. Do not test third-party endpoints or production services without permission.

## Handling evaluations safely

- Treat task packs, model outputs, and result directories as untrusted data.
- Run only against endpoints you are authorized to use. Keep credentials in environment variables rather than task files or command-line values.
- Result directories contain exact prompts and provider responses. Review their contents before sharing; they are not automatically sanitized exports.
- Saved receipts support integrity checks and offline regrading. They do not authenticate the identity of a remote model or prove execution on an untrusted machine.

The current runner collects direct answers. It does not execute generated code, load executable graders, or provide a code-execution sandbox. Any future execution support needs its own containment review; using a container alone would not establish that it is safe.
