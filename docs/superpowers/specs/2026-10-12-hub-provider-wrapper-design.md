# Hub provider wrapper

Date: 2026-10-12. Status: design approved in conversation, spec awaiting review.

## Goal

People know Hugging Face. When they type `gpt2` or `meta-llama/Llama-3.1-8B`, they mean
huggingface.co, and that must keep working unchanged in a workspace. Our hub
(`docs/superpowers/specs/2026-10-12-model-data-hub-design.md`) is reached by naming it, never by
redirecting Hugging Face traffic through us.

A thin wrapper takes a `provider` option. The default is `huggingface`. `kloudlite` sends the same
call to our hub, with our credential and a separate cache. Agents learn the wrapper from the
`kloudlite` skill so they pick the right provider without being told each time.

Success: in a workspace, with `HF_ENDPOINT` unset,

- `kl hf download openai-community/gpt2` and the Python `hub.download("openai-community/gpt2")`
  fetch from huggingface.co;
- `kl hf --provider kloudlite download karthik1729/gpt2-drill` and
  `hub.download("karthik1729/gpt2-drill", provider="kloudlite")` fetch from our hub with no
  token typed by the person;
- `from_pretrained(<returned path>)` loads the model;
- an agent asked to "use the model in our hub" picks `provider="kloudlite"` from the skill alone.

## Decisions (owner, 2026-10-12)

1. **No proxy.** The pull-through proxy design (owner-based routing to huggingface.co, token
   pass-through, sign-up name guard, `kl.` owner marker) is dropped. Two owners with the same name,
   one on Hugging Face and one here, never collide because the caller names the provider.
2. **Hugging Face is the default provider** everywhere: CLI, Python, skill.
3. **Workspaces do not preset `HF_ENDPOINT`.** Plain `huggingface_hub`, `transformers` and
   `datasets` behave exactly as on a laptop.
4. **The wrapper knowledge lives in the agents' `kloudlite` skill**, both copies (workspace agent
   and laptop plugin).

Evidence behind the decisions, verified 2026-10-12 against huggingface_hub source:

- `validate_repo_id` rejects a second `/` and `--`, so a marked repo id (`hf/x/y`, `kl--x/y`)
  cannot work; only an explicit provider separates the two hubs cleanly.
- `hf_hub_download`, `snapshot_download` and `HfApi` all take `endpoint=` and `token=` per call, so
  the wrapper needs no global state.
- Xet works with a per-call endpoint: our resolve answers with an `xet-auth` link on our own base
  (`crates/hub/src/resolve.rs:141`), and `utils/_xet.py` uses the passed endpoint for the token
  refresh.

Out of scope: the proxy, storing a person's Hugging Face token, mirroring or caching Hugging Face
content, a `from_pretrained` wrapper per framework, gated repos on our hub.

## 1. Pieces

| Piece | Where | What |
|---|---|---|
| Hub accepts the workspace credential | `crates/hub/src/auth.rs` | PAT miss falls through to the registry JWT |
| `KL_HUB_URL` in the workspace | `crates/workspaces/src/k8s/` | Hub base URL, threaded like `KL_REGISTRY_HOST` |
| `kl hf` | `bins/kl/src/hf.rs` | Runs the `hf` CLI with the provider's endpoint, token and cache |
| Python package | `sdk/python/kloudlite/` | `kloudlite.hub`: `download`, `download_file`, `api` |
| Skill text | both `skills/kloudlite/SKILL.md` | "Models and datasets" section |

### Provider table

One table, the same in Rust and Python:

| Provider | Endpoint | Token | Cache |
|---|---|---|---|
| `huggingface` (default) | unchanged (`HF_ENDPOINT` or huggingface.co) | unchanged (`HF_TOKEN` or `hf auth login`) | unchanged (`~/.cache/huggingface/hub`) |
| `kloudlite` | `$KL_HUB_URL` | `$KL_HUB_TOKEN`, else `/etc/kloudlite/ssh/registry-token` | `~/.cache/kloudlite/hub` |

`huggingface` passes nothing: the wrapper adds no behaviour to the default path, so a person's own
Hugging Face setup (token, mirror, cache) keeps working.

**Separate cache is load-bearing.** The Hugging Face cache keys a snapshot by repo id only
(`models--acme--x`). Without a separate `cache_dir`, `acme/x` from our hub and `acme/x` from
Hugging Face would share one cache entry and one would be served as the other. `~/.cache/` is in
the global gitignore and travels with the home volume, so a downloaded model survives restarts.

An unknown provider is an error naming both valid values. `kloudlite` with no `KL_HUB_URL` is an
error saying the hub address is missing (outside a workspace: set `KL_HUB_URL`). No token file and
no `KL_HUB_TOKEN` is anonymous, which still reads public repos.

## 2. Server: the hub accepts the workspace credential

Today `kloudlite_hub::auth::caller` accepts only a PAT (`owner_for_token`). A workspace holds no
PAT; it holds `registry-token`, a JWT minted every `KEYS_RESYNC_SECS` by
`crates/workspaces/src/api/workspaces/keys.rs` (`mint_registry(owner, "*", 86_400)`).

Change: when `owner_for_token` misses, verify the token with the registry's
`verify_registry_token(&app.jwt, token)`, exactly as `crates/registry/src/auth.rs` does for Basic.

- `RegistryToken::Owner(o)`: the caller is `o`; for Basic the user name must match (git's `x`
  placeholder allowed, as today).
- Anything else (anonymous-scope token, bad signature, expired): 401 `bad_credential()`, as a bad
  PAT is today. A presented credential that fails is a refusal, never anonymity.
- Authorization is unchanged: owner, `may_act`, or public repo. The token's `"*"` scope is never
  trusted; the hub re-checks the repo, as the registry re-checks the image.

The JWT verifier is called through `kloudlite_registry` (already a dependency of the server, not
of `crates/hub`). If `crates/hub` cannot depend on `crates/registry` without a cycle, move
`verify_registry_token` and `RegistryToken` to `crates/core` and re-export from the registry so its
paths do not change.

`// ponytail: one owner credential for registry and hub; split audiences if a hub-only token is
ever needed.`

## 3. Workspace: `KL_HUB_URL`

`KL_HUB_URL` (for example `https://hub.khost.dev`) joins the workspace container env next to
`KL_REGISTRY_HOST` in `crates/workspaces/src/k8s/workspace.rs`, and the SSH `SetEnv` line in
`secrets.rs::login_env`. The agent reads it from `WS_HUB_URL` (beside `WS_REGISTRY_HOST` in
`bins/agent/src/lib.rs`) and threads it through `k8s::PodContext` as `hub_url`, exactly like
`registry_host`. The bench does not get it (a bench has no hands).

Unset in a region without a hub: `kl hf --provider kloudlite` and `hub.download(...,
provider="kloudlite")` fail with the "hub address is missing" error.

## 4. `kl hf`

```
kl hf [--provider huggingface|kloudlite] <hf arguments...>
```

Runs the `hf` CLI (from `huggingface_hub`) with the rest of the arguments untouched. For
`kloudlite` it sets, in the child only: `HF_ENDPOINT=$KL_HUB_URL`, `HF_TOKEN=<token>` (when there
is one) and `HF_HUB_CACHE=~/.cache/kloudlite/hub`. For `huggingface` it sets nothing. Exit code
is `hf`'s.

So every `hf` verb works against either hub with no wrapper code per verb: `download`, `upload`,
`repo create`, `ls`.

- `hf` not on `PATH`: error "hf not found. Add the package with `kl pkg add
  python3Packages.huggingface-hub`." Do not install it silently.
- The token never appears in argv, logs or errors (env only).
- `--provider` must come before the `hf` arguments (`trailing_var_arg`). A `--provider` among
  the `hf` arguments is refused with an error saying to put it first, so a misplaced flag never
  sends the call to Hugging Face silently (owner decision, 2026-10-12).
- `kloudlite` with no kloudlite token is anonymous and sends no token at all: `HF_TOKEN` removed,
  `HF_HUB_DISABLE_IMPLICIT_TOKEN=1` (Python: `token=False`), so a person's Hugging Face token
  never reaches our hub. Verified in huggingface_hub 2.2.0 `get_token_to_send`.

File `bins/kl/src/hf.rs`, one `Cmd::Hf` variant in `main.rs`.

## 5. Python package `kloudlite.hub`

Location `sdk/python/` (`pyproject.toml`, `kloudlite/__init__.py`, `kloudlite/hub.py`). Depends
only on `huggingface_hub`. About 50 lines:

```python
from kloudlite import hub

path = hub.download("openai-community/gpt2")                              # Hugging Face
path = hub.download("acme/my-model", provider="kloudlite")                # our hub
f = hub.download_file("acme/my-model", "config.json", provider="kloudlite")
hub.api("kloudlite").upload_folder(repo_id="acme/my-model", folder_path="out/")
```

- `download(repo_id, provider="huggingface", **kw)` calls `snapshot_download` and returns the
  local path.
- `download_file(repo_id, filename, provider="huggingface", **kw)` calls `hf_hub_download`.
- `api(provider="huggingface")` returns an `HfApi` bound to the provider's endpoint and token,
  for uploads and everything else.
- Extra keyword arguments pass through (`repo_type="dataset"`, `revision=...`). A caller's own
  `cache_dir`, `endpoint` or `token` wins over the provider's.
- Loading is the framework's job: `AutoModel.from_pretrained(path)`, `load_dataset(path)`.

### Distribution

The package is published to PyPI as `kloudlite` (fallback `kloudlite-hub` if the name is taken),
version 0.1.0. Publishing is a public, outward-facing step and is done by the owner; the spec only
makes the package ready (`python -m build` produces a wheel). Until it is published, workspaces
install it with `pip install "kloudlite @ git+<repo url>#subdirectory=sdk/python"`.

## 6. Skills

Add a section "Models and datasets" to both copies of the `kloudlite` skill:

- `harness/packages/agent/claude-plugin/skills/kloudlite/SKILL.md` (workspace agents);
- `harness/laptop-plugin/plugin/skills/kloudlite/SKILL.md` (laptop plugin).

Written in ASD-STE100 like the rest of the skill, after "Repos and images". Content:

1. There are two model hubs: Hugging Face and the Kloudlite hub. Hugging Face is the default.
2. A model or dataset name alone (for example `gpt2` or `meta-llama/Llama-3.1-8B`) means Hugging
   Face. Use the usual tools without a provider.
3. Use the Kloudlite hub only when the person says "our hub", "Kloudlite hub", "our model", or
   names a repo that the person or the team uploaded here.
4. Do not set `HF_ENDPOINT`. Use `kl hf --provider kloudlite ...` in a shell, or
   `hub.download(..., provider="kloudlite")` in Python.
5. To load a model from the Kloudlite hub, download it first, then pass the local path to
   `from_pretrained` or `load_dataset`. A repo name in `from_pretrained` always goes to Hugging Face.
6. Upload trained models to the Kloudlite hub under the person's name or the team's name. Do not
   upload to Hugging Face unless the person tells you to.
7. Do not put a Hugging Face token in a file, a commit or a message. The Kloudlite hub needs no
   token in a workspace.

## 7. Testing and rollout

### Tests

- `crates/hub`: `caller` accepts a valid registry JWT (Bearer and Basic with matching user and
  with `x`), refuses an expired or foreign-signed JWT with 401, refuses a JWT whose Basic user
  differs, and still treats no header as anonymous.
- `crates/workspaces`: the workspace env and `login_env` carry `KL_HUB_URL`.
- `bins/kl`: unit test of the child env per provider: `huggingface` adds nothing; `kloudlite` sets
  the three variables; a missing `KL_HUB_URL` errors; an unknown provider errors. No test spawns
  `hf`.
- `sdk/python`: `python -m unittest` with `snapshot_download`/`hf_hub_download`/`HfApi`
  monkeypatched: arguments per provider, caller overrides win, unknown provider raises.
- `tests/hub_e2e.sh`: add a run with `HF_ENDPOINT` unset that uses `kl hf --provider kloudlite
  download` and the Python `hub.download(..., provider="kloudlite")`, and checks the file lands in
  `~/.cache/kloudlite/hub`, not the Hugging Face cache.

### Rollout

Server, api and agent images ship together (hub auth, env). Then the live drill in a workspace with
`HF_ENDPOINT` unset:

1. `kl hf download openai-community/gpt2 config.json` comes from huggingface.co.
2. `kl hf --provider kloudlite download karthik1729/gpt2-drill` with no token typed.
3. Python `hub.download("karthik1729/gpt2-drill", provider="kloudlite")`, then
   `from_pretrained(path)` generates text.
4. An agent session asked "load the gpt2-drill model from our hub" uses the provider without being
   told how.

### Done

All four drill steps pass, both skills carry the section, and the package wheel builds.
