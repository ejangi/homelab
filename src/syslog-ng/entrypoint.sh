#!/bin/sh
set -eu
mkdir -p /var/log/orbi /var/lib/syslog-ng
envsubst '${SYSLOG_LAN_IP}' < /etc/syslog-ng/template.conf > /etc/syslog-ng/syslog-ng.conf
syslog-ng --no-caps --syntax-only
syslog-ng -F --no-caps &
pid=$!
trap 'kill "$pid"; wait "$pid"; exit' TERM INT
while kill -0 "$pid" 2>/dev/null; do
    logrotate --state /var/log/orbi/.logrotate.state /etc/logrotate-orbi.conf
    sleep 60 &
    wait $! || true
done
wait "$pid"
