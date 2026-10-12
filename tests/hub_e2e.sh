#!/usr/bin/env bash
# Real clients against a local server: huggingface_hub + hf_xet, git + git-lfs, git-xet,
# transformers. Each section runs on its own; a section whose client is missing prints
# `skip: <section>: <client> missing` and the rest still run.
# Exit: 0 = every section that ran passed; 77 = nothing could run (python/venv/pip/git missing);
# anything else = a section failed. Set HUB_E2E_LOG=<file> to keep a copy of the server log.
set -euo pipefail
cd "$(dirname "$0")/.."

for c in python3 git; do
  command -v "$c" >/dev/null || { echo "skip: $c not installed"; exit 77; }
done

ROOT=$(pwd)/.local/hub-e2e
rm -rf "$ROOT" && mkdir -p "$ROOT"
SERVER=""
cleanup() {
  [ -n "$SERVER" ] && kill "$SERVER" 2>/dev/null || true
  [ -n "${HUB_E2E_LOG:-}" ] && cp "$ROOT/server.log" "$HUB_E2E_LOG" 2>/dev/null || true
  rm -rf "$ROOT"
}
trap cleanup EXIT

python3 -m venv "$ROOT/venv" \
  && "$ROOT/venv/bin/pip" -q install 'huggingface_hub[hf_xet]' transformers >"$ROOT/pip.log" 2>&1 \
  || { echo "skip: python venv or pip install failed (offline?)"; exit 77; }
PY="$ROOT/venv/bin/python"
# The SDK lives in its own repo; override KL_SDK_PIP to test a local checkout.
"$ROOT/venv/bin/pip" -q install "${KL_SDK_PIP:-kloudlite}" >>"$ROOT/pip.log" 2>&1 \
  || { echo "skip: kloudlite sdk install failed"; exit 77; }

BIN=${CARGO_TARGET_DIR:-target}/debug/kloudlite
cargo build -q --bin kloudlite --bin kl
export BIN
KL=${KL_BIN:-$(dirname "$BIN")/kl}

HUB_PORT=18081 CAS_PORT=18082
export KLOUDLITE_S3_URL="file://$ROOT/store"
export KLOUDLITE_HUB_ADDR=127.0.0.1:$HUB_PORT KLOUDLITE_CAS_ADDR=127.0.0.1:$CAS_PORT
export KLOUDLITE_HUB_URL=http://127.0.0.1:$HUB_PORT KLOUDLITE_CAS_URL=http://127.0.0.1:$CAS_PORT
export KLOUDLITE_HTTP_ADDR=127.0.0.1:18080 KLOUDLITE_PEER_ADDR=127.0.0.1:18083
export KLOUDLITE_SSH_ADDR=127.0.0.1:12222 KLOUDLITE_HOST_KEY="$ROOT/host_key"

# Same user + PAT bootstrap as the registry script, minted before the server opens the store.
source tests/lib/e2e_user.sh
TOKEN=$(e2e_user alice)

"$BIN" serve >"$ROOT/server.log" 2>&1 &
SERVER=$!
up=""
for _ in $(seq 100); do
  [ "$(curl -s -o /dev/null -w '%{http_code}' "$KLOUDLITE_HUB_URL/api/whoami-v2" || true)" = 401 ] && { up=1; break; }
  sleep 0.2
done
[ -n "$up" ] || { echo "server did not come up"; tail -20 "$ROOT/server.log"; exit 1; }
export HF_ENDPOINT=$KLOUDLITE_HUB_URL HF_TOKEN=$TOKEN HF_HUB_DISABLE_TELEMETRY=1

head -c $((200 * 1024 * 1024)) /dev/urandom >"$ROOT/weights.bin"
mkdir -p "$ROOT/up" && cp "$ROOT/weights.bin" "$ROOT/up/model.safetensors"
cat >"$ROOT/up/config.json" <<'JSON'
{"model_type": "gpt2", "n_layer": 1, "n_head": 1, "n_embd": 8, "vocab_size": 16, "n_positions": 8}
JSON

sec_upload() {
  "$PY" - "$ROOT" <<'PYEOF'
import filecmp, sys
from huggingface_hub import HfApi, snapshot_download
root = sys.argv[1]
api = HfApi()
api.create_repo("alice/m")
api.upload_folder(repo_id="alice/m", folder_path=f"{root}/up")
d = snapshot_download("alice/m", cache_dir=f"{root}/cache")
assert filecmp.cmp(f"{d}/model.safetensors", f"{root}/weights.bin", shallow=False)
PYEOF
}

sec_dedup() {
  local before after
  before=$(find "$ROOT/store" -path '*xet/alice/xorbs/*' -type f | wc -l)
  "$PY" - "$ROOT" <<'PYEOF'
import sys
from huggingface_hub import HfApi
HfApi().upload_file(path_or_fileobj=f"{sys.argv[1]}/weights.bin", path_in_repo="copy.bin", repo_id="alice/m")
PYEOF
  after=$(find "$ROOT/store" -path '*xet/alice/xorbs/*' -type f | wc -l)
  [ "$before" -gt 0 ] || { echo "FAIL: no xorb files found under the store"; return 1; }
  [ "$before" = "$after" ] || { echo "FAIL: dedup stored $((after - before)) new xorb files"; return 1; }
}

sec_lfs_clone() {
  command -v git-lfs >/dev/null || return 77
  git -c credential.helper= \
    -c filter.lfs.process="git-lfs filter-process" -c filter.lfs.smudge="git-lfs smudge -- %f" \
    -c filter.lfs.clean="git-lfs clean -- %f" -c filter.lfs.required=true \
    clone -q "http://alice:$TOKEN@127.0.0.1:$HUB_PORT/alice/m" "$ROOT/clone"
  cmp "$ROOT/clone/model.safetensors" "$ROOT/weights.bin"
}

sec_transformers() {
  "$PY" -c 'from transformers import AutoConfig; assert AutoConfig.from_pretrained("alice/m").n_embd == 8'
}

sec_xet_push() {
  command -v git-xet >/dev/null || return 77
  [ -d "$ROOT/clone" ] || return 77
  ( cd "$ROOT/clone" && git xet install --local >/dev/null \
    && head -c $((8 * 1024 * 1024)) /dev/urandom >pushed.bin && git lfs track pushed.bin >/dev/null \
    && git add .gitattributes pushed.bin && git -c user.name=a -c user.email=a@x commit -qm push && git push -q )
  "$PY" - "$ROOT" <<'PYEOF'
import filecmp, sys
from huggingface_hub import hf_hub_download
p = hf_hub_download("alice/m", "pushed.bin", cache_dir=f"{sys.argv[1]}/cache2")
assert filecmp.cmp(p, f"{sys.argv[1]}/clone/pushed.bin", shallow=False)
PYEOF
}

sec_provider() {
  [ -x "$KL" ] || return 77
  command -v hf >/dev/null || "$ROOT/venv/bin/hf" --help >/dev/null 2>&1 || return 77
  # As in a workspace: nothing presets HF_ENDPOINT, and the provider is named per call.
  ( unset HF_ENDPOINT HF_TOKEN
    export HOME="$ROOT/home" KL_HUB_URL="$KLOUDLITE_HUB_URL" KL_HUB_TOKEN="$TOKEN" PATH="$ROOT/venv/bin:$PATH"
    mkdir -p "$HOME"
    "$KL" hf --provider kloudlite download alice/m config.json >/dev/null
    ls "$HOME/.cache/kloudlite/hub"/models--alice--m >/dev/null
    [ ! -e "$HOME/.cache/huggingface/hub/models--alice--m" ]
    "$PY" - <<'PYEOF'
import os
from kloudlite import hub
p = hub.download_file("alice/m", "config.json", provider="kloudlite")
assert p.startswith(os.path.expanduser("~/.cache/kloudlite/hub")), p
PYEOF
  )
}

failed=0 summary=""
# Subshell with its own set -e, run outside any `if`/`||` so the -e actually applies.
run() { # run <label> <fn> <client for the skip line>
  local rc
  echo "== $1"
  set +e; ( set -e; "$2" ); rc=$?; set -e
  case $rc in
    0) summary+="pass: $1"$'\n' ;;
    77) echo "skip: $1: $3 missing"; summary+="skip: $1: $3 missing"$'\n' ;;
    *) echo "FAIL: $1"; summary+="FAIL: $1"$'\n'; failed=1 ;;
  esac
}
run upload_folder+snapshot_download sec_upload huggingface_hub
run dedup sec_dedup huggingface_hub
run git-lfs-clone sec_lfs_clone git-lfs
run AutoConfig sec_transformers transformers
run git-xet-push sec_xet_push git-xet
run provider-wrapper sec_provider kl

echo "---- summary"; printf '%s' "$summary"
[ "$failed" = 0 ] && echo PASS
exit "$failed"
