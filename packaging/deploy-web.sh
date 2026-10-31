#!/bin/sh
# LOCAL tool: build the web app and deploy it to Cloudflare Pages (project wordcraft-web,
# word.suseona.com). Cloudflare Pages caps static assets at 25 MiB and the release wasm is
# ~44 MB, so it travels gzip-compressed (.wasm.bin) and a small loader in index.html
# inflates it with DecompressionStream before booting — the same transform the site's
# earlier manual deploy used. The loader is deployment glue only; all app code stays Rust.
#
# The index.html patching runs through python3 (quoting the loader's template literals
# safely through shell/perl mangles them).
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

loader=$(mktemp)
trap 'rm -f "$loader"' EXIT
cat > "$loader" <<'EOF'
// The wasm travels gzip-compressed (Cloudflare Pages caps assets at 25 MiB); inflate it in the browser before booting.
const res = await fetch('./WC_WASM_BIN');
if (!res.ok) throw new Error(`failed to fetch Wasm: ${res.status} ${res.statusText}`);
let packed = new Uint8Array(await res.arrayBuffer());
const gz = packed[0] === 0x1f && packed[1] === 0x8b; // an intermediary already decoded it?
const bytes = gz
  ? new Uint8Array(await new Response(new Blob([packed]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer())
  : packed;
const wasm = await init({ module_or_path: bytes });
EOF

index="$dist/index.html"
python3 - "$index" "$name" "$loader" <<'PY'
import re, sys
path, name, loader = sys.argv[1:4]
html = open(path).read()
loader = open(loader).read().rstrip("\n").replace("WC_WASM_BIN", name + ".bin")
init_re = re.compile(r"const wasm = await init\(\{ module_or_path: '\./[\w-]+_bg\.wasm' \}\);")
html, n = init_re.subn(lambda _: loader, html)
preload_re = re.compile(r'<link rel="preload" href="[^"]*_bg\.wasm"[^>]*>')
html, m = preload_re.subn("", html)
if n != 1 or m > 1:
    sys.exit(f"deploy-web: patch failed (init={n}, preload={m})")
open(path, "w").write(html)
PY

# sanity: no raw wasm left, loader present
grep -q "$name.bin" "$index" || { echo "deploy-web: loader patch failed" >&2; exit 1; }
ls "$dist"/*.wasm >/dev/null 2>&1 && { echo "deploy-web: raw wasm still present" >&2; exit 1; }

echo "deploy-web: deploying $dist ($(du -sh "$dist" | cut -f1))"
wrangler pages deploy "$dist" --project-name wordcraft-web --branch main
