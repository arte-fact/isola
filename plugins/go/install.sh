echo ">>> Installing Go..."
GO_VERSION=$(curl -sL "https://go.dev/dl/?mode=json" | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['version'])" 2>/dev/null || echo "go1.23.6")
curl -sL "https://go.dev/dl/${GO_VERSION}.linux-amd64.tar.gz" | tar -C /usr/local -xzf -
echo 'export PATH="/usr/local/go/bin:$PATH"' >> /etc/profile.d/go.sh
