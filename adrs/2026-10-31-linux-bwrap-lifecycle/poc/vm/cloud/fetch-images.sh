#!/usr/bin/env bash
set -euo pipefail

dir=${1:-/tmp/opencode/images}
mkdir -p "$dir"

fetch() {
    local name=$1 url=$2 sha=$3
    if [[ -f "$dir/$name" ]] && echo "$sha  $dir/$name" | sha256sum -c - >/dev/null 2>&1; then
        echo "$name: cached and verified"
        return
    fi
    echo "$name: downloading $url"
    curl -L --fail --retry 2 -o "$dir/$name" "$url"
    echo "$sha  $dir/$name" | sha256sum -c -
}

fetch ubuntu-noble.img \
    https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img \
    04e8f2461a16a44e78a370284471f29616cdd9baf1893df25807b71f985b6c94

fetch fedora44.img \
    https://download.fedoraproject.org/pub/fedora/linux/releases/44/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2 \
    28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f
