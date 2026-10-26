#!/bin/sh
# Build and inspect a local test package; never install or restart audio services.
set -eu
driver_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$driver_dir/build-test-package.py" "$@"
