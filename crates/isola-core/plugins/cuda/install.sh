#!/bin/bash
set -euo pipefail

# cuda plugin — installs the CUDA toolkit (nvcc + headers + libcudart).
#
# The matching userspace *driver* libraries (libcuda, libnvidia-*) are
# bind-mounted from the host at sandbox entry — they must match the running
# host kernel module exactly — so only the version-independent toolkit is
# installed here. Requires the NVIDIA device nodes declared in plugin.yaml.

echo ">>> Installing CUDA toolkit (nvcc)..."
export DEBIAN_FRONTEND=noninteractive
apt-get install -y --no-install-recommends nvidia-cuda-toolkit
echo "CUDA toolkit installed."
