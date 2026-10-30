# stow usage

This guide covers the full key-value API and some coordination recipes. For a quick start, see the [README](README.md).

## Server options

```sh
stow serve                          # listens on 127.0.0.1:7319
stow serve --port 8080
stow serve --max-value-size 65536   # default is 1024 bytes
stow -q serve                       # no status output; errors still print
stow serve --save-file ./stow.bin   # keep state across restarts
```

## Key-value API

Values are limited to 1024 bytes by default; change it with `stow serve --max-value-size <BYTES>`. Larger values get a `413` response. Keys are 1 to 32 bytes and may contain `/`.

## Save file

Without `--save-file`, stow never touches disk. With `stow serve --save-file <PATH>`, stow loads the file at startup and writes it once on SIGINT or SIGTERM. Missing parent directories are created.

- A missing file starts an empty store.
- A file with bad magic bytes, an unknown format version, or a corrupt payload stops startup with exit code 1 and an error naming the file. stow does not overwrite it.
- Remaining TTLs are preserved; entries that expired while stopped are dropped.
- The write is atomic (temp file, `fsync`, rename), so a failed save leaves the previous file intact. SIGKILL or a crash skips the save and loses changes since the last start.

```sh
# set (raw body, or JSON)
curl -X PUT --data-binary 'hello' http://127.0.0.1:7319/api/v1/set/greeting
curl -X POST -H 'Content-Type: application/json' \
  -d '{"key": "lock/build", "value": "agent-1"}' http://127.0.0.1:7319/api/v1/set

# get (raw bytes, or JSON with key, value, and version)
curl http://127.0.0.1:7319/api/v1/get/greeting
curl 'http://127.0.0.1:7319/api/v1/get/greeting?format=json'

# list (prefix is optional)
curl 'http://127.0.0.1:7319/api/v1/keys?prefix=lock/'

# delete
curl -X DELETE http://127.0.0.1:7319/api/v1/delete/greeting
```

Set and delete accept conditions that are checked atomically with the write:

| Parameter | Applies to | Effect |
|-----------|-----------|--------|
| `if_absent=true` | set | Write only if the key is absent; otherwise `409`. |
| `if_value=<v>` | set, delete | Proceed only if the current value is exactly `<v>`; `409` on mismatch, `404` if the key is absent. |
| `ttl=<seconds>` | set | Expire the key after 1 to 86400 seconds. A set without `ttl` clears any existing TTL. |

`if_absent` and `if_value` cannot be combined. A `409` response includes the blocking entry: `{"error":{"code":"conflict",...},"current":{"value","version","ttl_remaining_ms"}}` (`value` is `null` if it is not UTF-8). `GET ...?format=json` includes `ttl_remaining_ms` (`null` when the key has no TTL). Expired keys are treated as absent immediately and freed by a sweep that runs every second. Expiry uses a monotonic clock.

Errors use one envelope: `{"error":{"code":"not_found","message":"..."}}`. `format=json` on a value that is not valid UTF-8 returns `406`; fetch it without `format`.

## Coordination recipes

Each condition is checked and applied in one step under the store lock, so exactly one concurrent caller wins.

**Consensus (first writer wins).** Each caller proposes a value; losers get `409` and adopt the winner's value from `current`.

```sh
curl -s -w '\n%{http_code}\n' -X PUT --data-binary 'plan-b' \
  'http://127.0.0.1:7319/api/v1/set/decision?if_absent=true'
# 200 {"key":"decision","version":1}   you won
# 409 {"error":{...},"current":{"value":"plan-a",...}}   adopt "plan-a"
```

**Mutex with a lease.** Use a unique owner token as the value and a TTL so a crashed holder cannot block everyone.

```sh
OWNER=agent-$$
# acquire (200 = you hold the lock, 409 = someone else does)
curl -X PUT --data-binary "$OWNER" 'http://127.0.0.1:7319/api/v1/set/lock/build?if_absent=true&ttl=60'

# renew before the lease runs out; 409 or 404 means you lost it
curl -X PUT --data-binary "$OWNER" "http://127.0.0.1:7319/api/v1/set/lock/build?if_value=$OWNER&ttl=60"

# release; only deletes if you still own it
curl -X DELETE "http://127.0.0.1:7319/api/v1/delete/lock/build?if_value=$OWNER"
```

**Back off on 409.** There is no blocking acquire; poll with exponential back-off and jitter.

```sh
delay=1
until curl -sf -X PUT --data-binary "$OWNER" \
    'http://127.0.0.1:7319/api/v1/set/lock/build?if_absent=true&ttl=60' >/dev/null; do
  sleep "$delay"
  delay=$(( delay < 30 ? delay * 2 : 30 ))
done
```
