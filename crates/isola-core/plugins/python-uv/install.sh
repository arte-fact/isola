echo ">>> Installing Python + uv..."
apt-get install -y python3 python3-venv || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
curl -LsSf https://astral.sh/uv/install.sh | sh
