# Sourced by e2e scripts. `e2e_user <name>` prints a fresh PAT for owner <name>, minted with
# `kloudlite admin add-token` against whatever store the caller exported (KLOUDLITE_S3_URL and
# friends). BIN must point at the built `kloudlite` binary. Only the token goes to stdout.
e2e_user() {
  "${BIN:?set BIN to the kloudlite binary}" admin add-token "$1"
}
