# n8n Homelab

This repository manages the local n8n automation environment and its supporting services. It treats reusable workflows and service integrations as stable internal capabilities.

## Language

**Matrix Service**:
The internal service that owns Matrix authentication, client-device state, encryption, and message delivery for n8n workflows.
_Avoid_: Matrix workflow, Matrix bot

**Matrix Sender Workflow**:
The reusable n8n sub-workflow that validates caller input and delegates a notification to the Matrix Service.
_Avoid_: Matrix Service, notification workflow

**Delivery Request**:
A request to post one message to a Matrix room, identified optionally by a caller-supplied idempotency key.
_Avoid_: Job, event

**Encryption Mode**:
The caller's explicit choice between encrypted delivery and plaintext delivery. Encrypted delivery is the default.
_Avoid_: Encryption fallback, automatic downgrade

**Setup**:
The authenticated administrative process that initializes the Matrix client device, manages optional verification, and explicitly enables room encryption.
_Avoid_: Send, delivery
