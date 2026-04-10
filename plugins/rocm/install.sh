#!/bin/bash
set -euo pipefail

# ROCm plugin — install AMD ROCm user-space stack.
# If ROCm is already present (e.g., baked into rootfs), this is a no-op.

ROCM_VERSION="${ROCM_VERSION:-6.4}"

if command -v rocminfo &>/dev/null; then
    echo "ROCm already installed: $(rocminfo 2>/dev/null | head -1 || echo 'present')"
    exit 0
fi

echo ">>> Installing ROCm ${ROCM_VERSION}..."

apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    wget \
    gnupg2 \
    ca-certificates

# Add ROCm repository
mkdir -p /etc/apt/keyrings
wget -q -O - https://repo.radeon.com/rocm/rocm.gpg.key | gpg --dearmor -o /etc/apt/keyrings/rocm.gpg
echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/rocm.gpg] https://repo.radeon.com/rocm/apt/${ROCM_VERSION} noble main" \
    > /etc/apt/sources.list.d/rocm.list

# Pin ROCm packages to avoid version conflicts
cat > /etc/apt/preferences.d/rocm-pin <<ROCMPIN
Package: *
Pin: release o=repo.radeon.com
Pin-Priority: 600
ROCMPIN

apt-get update -qq

# Install targeted ROCm components instead of the massive rocm-dev/rocm-libs
# meta-packages which frequently have broken dependencies.
apt-get install -y -qq --no-install-recommends \
    rocm-hip-runtime-dev \
    hip-dev \
    rocblas-dev \
    rocrand-dev \
    rocsolver-dev \
    rccl-dev \
    rocminfo \
    rocm-smi-lib \
    || {
        echo "Targeted install failed, trying minimal ROCm..."
        apt-get install -y -qq --no-install-recommends \
            rocm-hip-runtime-dev \
            rocminfo \
            || {
                echo "WARNING: ROCm installation failed. You may need to install manually."
                echo "Try: sudo apt-get install rocm-hip-runtime-dev"
                exit 0
            }
    }

# Set up environment for sandbox user
cat >> /home/sandbox/.bashrc << 'ROCMEOF'

# ROCm
export ROCM_PATH=/opt/rocm
export PATH="$ROCM_PATH/bin:$PATH"
export LD_LIBRARY_PATH="$ROCM_PATH/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
ROCMEOF

echo "ROCm ${ROCM_VERSION} installed."
