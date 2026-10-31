# stow usage

This guide covers the full key-value API and some coordination recipes. For a quick start, see the [README](README.md).

## Server options

```sh
stow serve                          # listens on 127.0.0.1:7319
stow serve --port 8080
stow serve --max-value-size 65536   # default is 1024 bytes
stow serve --openapi                # also serves the OpenAPI spec
stow -q serve                       # no status output; errors still print
stow serve --save-file ./stow.bin   # keep state across restarts
```

## OpenAPI spec

With `--openapi`, the server describes its own API as an OpenAPI 3.1 document. Without the flag the route returns `404`.

```sh
curl http://127.0.0.1:7319/api/v1/openapi.json
```

## Key-value API

Values are limited to 1024 bytes by default; change it with `stow serve --max-value-size <BYTES>`, up to 1048576 (1 MiB); larger settings are refused at startup. Values over the limit get a `413` response. Keys are 1 to 32 bytes and may contain `/`.

## Client commands

The `stow` binary can call its own API, so no `curl` is needed. Every client command accepts `--addr HOST:PORT` (default `127.0.0.1:7319`, or the `STOW_ADDR` environment variable).

```sh
stow set greeting hello                       # value "-" reads stdin
stow set decision plan-b --if-absent --ttl 60
stow get greeting                             # raw bytes on stdout
stow keys --prefix lock/                      # one key per line
stow del greeting --if-value hello
stow lock lock/build --ttl 60                 # prints the owner token
stow unlock lock/build --owner stow-1234
```

`set` takes `--ttl`, `--if-absent`, and `--if-value`, with the same meaning as the query parameters above. `del` takes `--if-value`.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Key not found, or any other failure (the message is on stderr) |
| 2 | Conflict: a condition did not hold (`409`) |
| 3 | Server unreachable |
| 4 | Timed out waiting |

### Waiting for a key

`get <key> --wait-for <condition>` polls until the condition holds, prints the value (nothing for `absent`), and exits 0.

| Condition | Holds when |
|-----------|-----------|
| `present` | The key exists and has not expired. |
| `absent` | The key does not exist. A delete and a TTL expiry both count. If it is already absent, the command exits at once. |
| `value=<v>` | The value equals `<v>` byte for byte, the same rule as `if_value`. |

```sh
stow get build-sha --wait-for present
stow get task-42/status --wait-for value=done --timeout 300
```

Without `--wait-for`, `get` returns immediately and exits 1 if the key is missing.

### Taking a lock

`lock <key> --ttl N` makes one attempt to write your owner token with `if_absent=true`. It prints the token and exits 0, or exits 2 if someone else holds the lock. Without `--owner`, the token is `stow-<pid>`.

`lock <key> --ttl N --wait` keeps trying until it holds the lock. It polls the key until it is absent, then makes the same atomic write; if another waiter wins that race, it goes back to waiting. The lease frees itself after the TTL if the holder never calls `unlock`.

```sh
stow lock lock/build --ttl 60 --wait
```

`get --wait-for` observes and `lock` acquires. Waiting for `absent` and then calling `lock` is racy, because every waiter wakes together and the losers get a conflict. Use `lock --wait` to take a mutex, and `get --wait-for` for state you do not plan to claim.

### Timeouts and polling

| Option | Default | Meaning |
|--------|---------|---------|
| `--timeout <seconds>` | 60 | Give up and exit 4 after this long. `0` waits indefinitely. |
| `--interval <ms>` | 250 | First pause between polls. It doubles after each poll up to 2000 ms. |

Each pause also gets up to 50% extra random delay, so waiters started together spread out instead of waking in lockstep. Each poll is a single `GET`, and at the default interval a waiter sends at most about four requests per second. If the server is unreachable during a wait, the command exits 3 at once without retrying.

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

**Back off on 409.** With `curl`, there is no blocking acquire; poll with exponential back-off and jitter. The `stow lock --wait` command does this for you (see [Client commands](#client-commands)).

```sh
delay=1
until curl -sf -X PUT --data-binary "$OWNER" \
    'http://127.0.0.1:7319/api/v1/set/lock/build?if_absent=true&ttl=60' >/dev/null; do
  sleep "$delay"
  delay=$(( delay < 30 ? delay * 2 : 30 ))
done
```
