FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends syslog-ng-core logrotate gettext-base tzdata && rm -rf /var/lib/apt/lists/*
COPY syslog-ng.conf /etc/syslog-ng/template.conf
COPY logrotate /etc/logrotate-orbi.conf
COPY entrypoint.sh /entrypoint.sh
RUN chmod 0644 /etc/logrotate-orbi.conf
ENTRYPOINT ["/bin/sh", "/entrypoint.sh"]
