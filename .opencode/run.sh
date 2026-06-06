#!/usr/bin/env bash
set -eu -o pipefail

export OPENCODE_CONFIG_DIR=$(pwd)/.opencode

exec opencode "$@"
