echo ">>> Installing Node.js..."
curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
apt-get install -y nodejs || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
