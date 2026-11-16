# Kloudlite SDK for Python

```sh
pip install kloudlite-sdk
```

```python
from kloudlite import Kloudlite

kl = Kloudlite()  # uses the credentials of `kloudlite login`
```

A snapshot push is a storage commit; a branch names one and moves like a git branch:

```python
kl.workspaces.take_snapshot(workspace="api", branch="wip-auth", labels=["auth"])
kl.volumes.list(label="auth")
kl.volumes.describe(owner="acme", name="api-home", description="API dev home")
kl.volumes.set_branch(owner="acme", name="api-home", branch="base", version=4)
kl.volumes.delete_branch(owner="acme", name="api-home", branch="wip-auth")
```

The distribution is `kloudlite-sdk`; the import name is `kloudlite`. The PyPI project named `kloudlite` is a different package.
