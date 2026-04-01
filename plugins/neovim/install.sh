echo ">>> Installing neovim..."
apt-get install -y --no-install-recommends neovim || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
