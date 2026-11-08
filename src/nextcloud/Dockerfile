FROM nextcloud:35-apache
# Match the existing host file owner without changing the external files.
RUN groupmod -g 1000 www-data && usermod -u 1000 -g 1000 www-data
