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
