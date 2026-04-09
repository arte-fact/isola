echo ">>> Installing fish shell..."
apt-get install -y --no-install-recommends fish || true
dpkg --configure -a --force-overwrite 2>/dev/null || true

# isola sandbox prompt marker (right prompt — avoids conflicts with OMF/fisher themes)
mkdir -p /etc/fish/conf.d
cat > /etc/fish/conf.d/isola.fish << 'ISOLA_FISH_EOF'
if set -q ISOLA_SANDBOX
    function fish_right_prompt
        echo -n (set_color cyan)"(isola:$ISOLA_SANDBOX)"(set_color normal)
    end
end
ISOLA_FISH_EOF
