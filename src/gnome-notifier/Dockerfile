FROM python:3.13-slim-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends libnotify-bin \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY scripts/n8n-gnome-notify.py /app/n8n-gnome-notify.py

EXPOSE 8788

CMD ["python3", "/app/n8n-gnome-notify.py"]
