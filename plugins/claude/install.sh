echo ">>> Installing Claude Code..."
curl -fsSL https://claude.ai/install.sh | bash
# Installer may target root or sandbox user depending on /etc/passwd detection
chmod +x /root/.local/bin/claude /home/sandbox/.local/bin/claude 2>/dev/null || true

# Seed Claude config to skip onboarding (login may still be required without claude-config)
mkdir -p /home/sandbox/.claude
if [ ! -f /home/sandbox/.claude/.claude.json ]; then
    printf '{"hasCompletedOnboarding":true}\n' > /home/sandbox/.claude/.claude.json
fi
chown -R sandbox:sandbox /home/sandbox/.claude 2>/dev/null || true
