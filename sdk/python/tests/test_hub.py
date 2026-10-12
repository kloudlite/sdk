import os
import tempfile
import unittest
from unittest import mock

from kloudlite import hub

HUB = {"KL_HUB_URL": "https://hub.khost.dev/", "KL_HUB_TOKEN": "tok"}


class Provider(unittest.TestCase):
    def call(self, fn, *a, env=None, **kw):
        with mock.patch.dict(os.environ, env or {}, clear=False), \
             mock.patch.object(hub, "snapshot_download", return_value="/p") as sd, \
             mock.patch.object(hub, "hf_hub_download", return_value="/f") as hd:
            fn(*a, **kw)
            return (sd if sd.called else hd).call_args

    def test_huggingface_passes_nothing(self):
        args = self.call(hub.download, "openai-community/gpt2", env=HUB)
        self.assertEqual(args.args, ("openai-community/gpt2",))
        self.assertEqual(args.kwargs, {})

    def test_kloudlite_sets_endpoint_token_and_cache(self):
        args = self.call(hub.download, "acme/m", provider="kloudlite", env=HUB)
        self.assertEqual(args.kwargs["endpoint"], "https://hub.khost.dev")
        self.assertEqual(args.kwargs["token"], "tok")
        self.assertEqual(args.kwargs["cache_dir"], os.path.expanduser("~/.cache/kloudlite/hub"))

    def test_download_file_passes_filename(self):
        args = self.call(hub.download_file, "acme/m", "config.json", provider="kloudlite", env=HUB)
        self.assertEqual(args.args, ("acme/m", "config.json"))

    def test_caller_overrides_win(self):
        args = self.call(hub.download, "acme/m", provider="kloudlite", env=HUB, cache_dir="/c", repo_type="dataset")
        self.assertEqual(args.kwargs["cache_dir"], "/c")
        self.assertEqual(args.kwargs["repo_type"], "dataset")

    def test_token_file_is_trimmed_and_empty_is_anonymous(self):
        with tempfile.NamedTemporaryFile("w", delete=False) as f:
            f.write("filetok\n")
        env = {"KL_HUB_URL": "https://hub.khost.dev", "KL_HUB_TOKEN": ""}
        with mock.patch.object(hub, "TOKEN_FILE", f.name):
            self.assertEqual(self.call(hub.download, "a/m", provider="kloudlite", env=env).kwargs["token"], "filetok")
            open(f.name, "w").close()
            # False, never None: None would let huggingface_hub send the person's own HF token.
            self.assertIs(self.call(hub.download, "a/m", provider="kloudlite", env=env).kwargs["token"], False)
        with mock.patch.object(hub, "TOKEN_FILE", "/nonexistent/registry-token"):
            self.assertIs(self.call(hub.download, "a/m", provider="kloudlite", env=env).kwargs["token"], False)
        os.unlink(f.name)

    def test_missing_address_and_unknown_provider(self):
        with mock.patch.dict(os.environ, {"KL_HUB_URL": ""}):
            with self.assertRaisesRegex(RuntimeError, "hub address is missing"):
                hub.download("a/m", provider="kloudlite")
        with self.assertRaisesRegex(ValueError, "huggingface.*kloudlite"):
            hub.download("a/m", provider="hugginface")

    def test_api_binds_endpoint_and_token(self):
        with mock.patch.dict(os.environ, HUB), mock.patch.object(hub, "HfApi") as api:
            hub.api("kloudlite")
            api.assert_called_once_with(endpoint="https://hub.khost.dev", token="tok")
            hub.api()
            api.assert_called_with()


if __name__ == "__main__":
    unittest.main()
