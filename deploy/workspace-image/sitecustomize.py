# On PYTHONPATH in every workspace (`login_env`). A pip manylinux wheel (torch) needs libstdc++,
# libgcc_s and zlib; a Nix Python's loader never searches /usr/lib, and a global LD_LIBRARY_PATH
# would hand Nix's newer libstdc++ to the image's Debian-glibc binaries. So a Nix Python loads the
# profile's copies here, once; a later dlopen of a wheel matches them by soname.
# ponytail: assumes the Python and the profile's libs share a glibc (same nixpkgs pin); a Python
# pinned to an older nixpkgs fails the preload silently and wheels fail as they did before.
import os
import sys


def _preload():
    if not os.path.realpath(sys.executable).startswith("/nix/store/"):
        return
    import ctypes

    for lib in ("libgcc_s.so.1", "libstdc++.so.6", "libz.so.1"):
        try:
            ctypes.CDLL("/nix/profile/current/lib/" + lib, mode=ctypes.RTLD_GLOBAL)
        except OSError:
            pass


def _chain():
    # Shadowing would drop the next sitecustomize on sys.path, and Nix's own one is what makes
    # `python3.withPackages` environments work.
    import importlib.machinery
    import importlib.util

    here = os.path.dirname(os.path.abspath(__file__))
    rest = [p for p in sys.path if os.path.abspath(p or ".") != here]
    spec = importlib.machinery.PathFinder.find_spec("sitecustomize", rest)
    if spec is None or spec.loader is None:
        return
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)


try:
    _preload()
    _chain()
except Exception:
    pass
