#!/usr/bin/env bash
# Populate fixtures/ with real camera files for golden tests.
# Local R5 Mark II corpus first; public CC0 samples (raw.pixls.us) for
# bodies we don't own.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p fixtures

CORPUS=/home/pc/temp/EOSR5_20251014/100EOSR5
if [ -d "$CORPUS" ]; then
  cp -n "$CORPUS/4P4A9687.CR3" fixtures/ 2>/dev/null || true
  cp -n "$CORPUS/4P4A1410.JPG" fixtures/ 2>/dev/null || true
fi

# CC0 CR2 sample (Canon EOS 5D Mark III) from raw.pixls.us
CR2_URL="https://raw.pixls.us/getfile.php/771/nice/Canon%20-%20EOS%205D%20Mark%20III.CR2"
if [ ! -s fixtures/5d3.CR2 ]; then
  curl -fSL -o fixtures/5d3.CR2 "$CR2_URL" \
    || { rm -f fixtures/5d3.CR2; echo "warn: CR2 sample download failed (offline?)"; }
fi

ls -l fixtures/
