#!/usr/bin/env bash
set -euo pipefail

echo ">>> Installing Google Chrome..."
# On Ubuntu 24.04, the chromium-browser apt package installs a snap stub that
# won't work inside a user namespace. Download the Google Chrome deb directly.
curl -fsSL https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb \
    -o /tmp/chrome.deb
apt-get install -y /tmp/chrome.deb || apt-get install -yf
rm -f /tmp/chrome.deb
echo ">>> Google Chrome installed. Run with: google-chrome --no-sandbox --remote-debugging-port=9222"
