# Agent instructions

## Project skills

Project-specific Codex skills are stored under `.agents/skills/`. When a task
matches one of these skills, read its `SKILL.md` before acting and follow its
instructions. Skills can also be invoked explicitly with `$skill-name`.

## n8n API access

When an agent needs to use the n8n API, load the local credentials from
`.env.agents`. It defines:

- `N8N_API_URL` — the n8n API base URL
- `N8N_API_KEY` — the API key

Do not print, log, paste, or commit `N8N_API_KEY`. Keep it in request headers
only. For shell commands, load the file without displaying it, for example:

```sh
set -a
. ./.env.agents
set +a
```

Use the API only when the task calls for changing or inspecting the n8n
instance. Never place the key in workflow JSON, source files, or command
arguments.

## Matrix notifications

When changing the Matrix Service or the Matrix Sender Workflow, read
the [Matrix notifications service and workflow specification](docs/specs/matrix-notifications.md).
The Rust service lives in `src/matrix/`, uses Postgres for Matrix/client state,
and is called by n8n at `MATRIX_SERVICE_URL` with `MATRIX_SERVICE_API_KEY`.

Matrix secrets (`MATRIX_PASSWORD`, `MATRIX_STORE_ENCRYPTION_KEY`, and
`MATRIX_SERVICE_API_KEY`) belong only in the local `.env`; keep them out of
source files, workflow JSON, command arguments, and logs. Run Matrix Service
migrations through Docker Compose, not manually against the database.
