#!/bin/bash
set -euo pipefail

# gpu plugin — ensure video/render groups exist and sandbox is a member.
# Works for AMD, NVIDIA, and Intel GPUs.
# This is minimal and unlikely to fail (no network, no package downloads).

echo ">>> Setting up GPU access..."

groupadd -f video
groupadd -f render
usermod -aG video,render sandbox

echo "GPU access configured (sandbox added to video, render groups)."
