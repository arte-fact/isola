echo ">>> Installing Claude Code..."
curl -fsSL https://claude.ai/install.sh | sh

# Create a wrapper that always runs with --dangerously-skip-permissions
cat > /usr/local/bin/claude-unchained << 'WRAPPER'
#!/bin/bash
exec claude --dangerously-skip-permissions "$@"
WRAPPER
chmod +x /usr/local/bin/claude-unchained
