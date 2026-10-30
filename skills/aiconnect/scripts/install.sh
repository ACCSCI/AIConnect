#!/bin/sh
set -eu
# Usage: sh install.sh --manifest URL --public-key BASE64_DER [--version VERSION]
command -v node >/dev/null 2>&1 || { echo 'Bootstrap requires Node.js 22.20+. Native AIConnect does not require Node.js.' >&2; exit 1; }
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
engine="$script_dir/install.mjs"
if [ ! -f "$engine" ]; then
 echo 'Download install.mjs beside this installer from the same trusted AIConnect release website, then rerun.' >&2
 exit 1
fi
exec node "$engine" "$@"
