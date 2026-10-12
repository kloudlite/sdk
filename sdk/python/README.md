# kloudlite (Python)

```python
from kloudlite import hub
path = hub.download("acme/my-model", provider="kloudlite")
```

Hugging Face is the default provider. `kloudlite` reads `KL_HUB_URL` and the workspace token.
Until the package is on PyPI: `pip install "kloudlite @ git+https://github.com/kloudlite/rustic-git#subdirectory=sdk/python"`.
Tests: `python -m unittest discover -s tests` (needs `huggingface_hub`).
