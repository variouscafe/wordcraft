#!/bin/sh
# LOCAL tool: build the web app and deploy it to Cloudflare Pages (project wordcraft-web,
# word.suseona.com). Cloudflare Pages caps static assets at 25 MiB and the release wasm is
# ~44 MB, so it travels gzip-compressed (.wasm.bin) and a small loader in index.html
# inflates it with DecompressionStream before booting — the same transform the site's
# earlier manual deploy used. The loader is deployment glue only; all app code stays Rust.
#
# Usage: packaging/deploy-web.sh [--skip-build]
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dist="$root/dist/web"

if [ "${1:-}" != "--skip-build" ]; then
  (cd "$root/apps/wordcraft-web" && trunk build --release)
fi

wasm=$(ls "$dist"/wordcraft-web-*_bg.wasm 2>/dev/null | head -n 1)
if [ -z "$wasm" ]; then
  echo "deploy-web: no *_bg.wasm in $dist — build first" >&2
  exit 1
fi
name=$(basename "$wasm")

# gzip (keep the transform deterministic; -n omits the timestamp)
gzip -9 -n -c "$wasm" > "$dist/$name.bin"
rm "$wasm"

# index.html: swap the direct init for the gzip loader, drop the now-dangling wasm preload.
index="$dist/index.html"
perl -0pi -e "s{const wasm = await init\(\{ module_or_path: '\\./\Q$name\E' \}\);}{// The wasm travels gzip-compressed (Cloudflare Pages caps assets at 25 MiB); inflate it in the browser before booting.\nconst res = await fetch('./$name.bin');\nif (!res.ok) throw new Error(\\`failed to fetch Wasm: \\\${res.status} \\\${res.statusText}\\`);\nlet packed = new Uint8Array(await res.arrayBuffer());\nconst gz = packed[0] === 0x1f \&\& packed[1] === 0x8b; // an intermediary already decoded it?\nconst bytes = gz\n  ? new Uint8Array(await new Response(new Blob([packed]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer())\n  : packed;\nconst wasm = await init({ module_or_path: bytes });}" "$index"
perl -0pi -e 's{<link rel="preload" href="[^"]*_bg\.wasm"[^>]*>}{}' "$index"

# sanity: no raw wasm left, loader present
grep -q "$name.bin" "$index" || { echo "deploy-web: loader patch failed" >&2; exit 1; }
ls "$dist"/*.wasm >/dev/null 2>&1 && { echo "deploy-web: raw wasm still present" >&2; exit 1; }

echo "deploy-web: deploying $dist ($(du -sh "$dist" | cut -f1))"
wrangler pages deploy "$dist" --project-name wordcraft-web --branch main
