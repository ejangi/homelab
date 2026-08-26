# GNOME execution-failure notifications

`n8n execution failure notifications` is the shared n8n Error Workflow. Every
active production workflow designates it as its **Error workflow**, so a failed
automatic execution sends its workflow name, failed node, error message, and
execution URL to the local GNOME desktop.

## Components

- n8n sends the notification to `GNOME_NOTIFICATION_SERVICE_URL` after an
  Error Trigger starts the shared workflow.
- The `gnome-notifier` Compose service runs `scripts/n8n-gnome-notify.py` and
  calls `notify-send` through the operator's GNOME D-Bus session.
- Its port is private to `n8n_backend`, so n8n calls it by the Docker-only
  address `http://gnome-notifier:8788`. Requests must present
  `GNOME_NOTIFICATION_SERVICE_API_KEY`.

The service requires an active GNOME login, since it mounts that user's D-Bus
socket. While the desktop session is logged out, the container cannot start;
n8n still records the failure in its Executions UI.

## Scope and testing

Error Workflows run when an automatic workflow that has selected this handler
fails. Manual editor executions do not trigger it. To test the desktop bridge,
run the Error Workflow manually: its Error Trigger supplies an example payload
when manually executed.
