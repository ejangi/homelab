# n8n Local Homelab

> Uses [n8n](https://github.com/n8n-io/n8n), serves using https via [caddy](https://github.com/caddyserver/caddy) and do 24 hour backups.

## Installation

Once you clone this repo, ensure you DON'T track the changes to your `.env` file:

```bash
git update-index --assume-unchanged .env
```

Then, edit the `.env` file with the credentials you intend to use.
