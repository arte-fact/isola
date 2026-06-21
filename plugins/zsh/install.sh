echo ">>> Installing zsh..."
apt-get install -y --no-install-recommends zsh || true
dpkg --configure -a --force-overwrite 2>/dev/null || true

# isola sandbox prompt marker (precmd hook — runs after ~/.zshrc so it survives theme overrides)
mkdir -p /etc/zsh
cat >> /etc/zsh/zshrc << 'ISOLA_ZSH_EOF'
if [[ -n "${ISOLA_SANDBOX:-}" ]]; then
    autoload -Uz add-zsh-hook
    _isola_precmd() {
        [[ "${PROMPT}" == *'(isola:'* ]] || PROMPT="%F{cyan}(isola:${ISOLA_SANDBOX})%f ${PROMPT}"
    }
    add-zsh-hook precmd _isola_precmd
fi
ISOLA_ZSH_EOF
