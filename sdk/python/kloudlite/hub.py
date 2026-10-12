"""Models and datasets from Hugging Face (the default) or the kloudlite hub.

    from kloudlite import hub
    path = hub.download("openai-community/gpt2")                  # Hugging Face
    path = hub.download("acme/my-model", provider="kloudlite")    # the kloudlite hub
    AutoModel.from_pretrained(path)

`huggingface` passes nothing, so a person's own Hugging Face token, mirror and cache keep working.
`kloudlite` passes the hub's endpoint, the workspace's token and a cache of its own: Hugging Face
keys a cache entry by repo id alone, so the same name from both hubs must not share one. Without a
kloudlite token the call sends none (`token=False`), never the person's Hugging Face token.
"""

import os

from huggingface_hub import HfApi, hf_hub_download, snapshot_download

PROVIDERS = ("huggingface", "kloudlite")
TOKEN_FILE = "/etc/kloudlite/ssh/registry-token"


def _token():
    t = os.environ.get("KL_HUB_TOKEN", "").strip()
    if t:
        return t
    try:
        with open(TOKEN_FILE) as f:
            return f.read().strip() or False
    except OSError:
        return False


def _options(provider):
    if provider == "huggingface":
        return {}
    if provider != "kloudlite":
        raise ValueError(f"unknown provider {provider!r}: use 'huggingface' or 'kloudlite'")
    url = os.environ.get("KL_HUB_URL", "").strip().rstrip("/")
    if not url:
        raise RuntimeError("hub address is missing: KL_HUB_URL is not set (a workspace sets it)")
    return {"endpoint": url, "token": _token(), "cache_dir": os.path.expanduser("~/.cache/kloudlite/hub")}


def download(repo_id, provider="huggingface", **kw):
    """Download a whole repo; returns the local path for `from_pretrained` or `load_dataset`."""
    return snapshot_download(repo_id, **{**_options(provider), **kw})


def download_file(repo_id, filename, provider="huggingface", **kw):
    """Download one file; returns its local path."""
    return hf_hub_download(repo_id, filename, **{**_options(provider), **kw})


def api(provider="huggingface"):
    """An `HfApi` bound to the provider, for uploads and everything else."""
    o = _options(provider)
    o.pop("cache_dir", None)
    return HfApi(**o)
