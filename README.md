# stow

Minimally viable KV storage for local use.

stow ships as a GitHub Release binary. It is not published to crates.io.

## Usage

```sh
stow serve                 # listens on 127.0.0.1:7319
stow serve --port 8080
stow -q serve              # no status output; errors still print
curl http://127.0.0.1:7319/api/v1/health
```

See [CONTRIBUTING.md](CONTRIBUTING.md) to build from source.
