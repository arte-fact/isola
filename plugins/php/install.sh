#!/bin/bash
set -euo pipefail

echo ">>> Installing PHP..."

apt-get update -qq
apt-get install -y -qq --no-install-recommends software-properties-common
add-apt-repository -y ppa:ondrej/php
apt-get update -qq

PHP_VERSION=8.4

apt-get install -y -qq --no-install-recommends \
    php${PHP_VERSION}-cli \
    php${PHP_VERSION}-common \
    php${PHP_VERSION}-curl \
    php${PHP_VERSION}-mbstring \
    php${PHP_VERSION}-xml \
    php${PHP_VERSION}-zip \
    php${PHP_VERSION}-mysql \
    php${PHP_VERSION}-sqlite3 \
    php${PHP_VERSION}-bcmath \
    php${PHP_VERSION}-intl \
    php${PHP_VERSION}-readline \
    unzip

echo ">>> Installing Composer..."
curl -sS https://getcomposer.org/installer | php -- --install-dir=/usr/local/bin --filename=composer

echo ">>> Installing Laravel Installer..."
su - sandbox -c "composer global require laravel/installer" || true

echo "PHP ${PHP_VERSION} + Composer installed."
