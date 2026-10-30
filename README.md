# stow
<!-- Everthing here is human-grade. No slop!-->
A small, local, in-memory key-value server. 

This service is meant for process coordination in **local** and **isolated** environments.  It does not suggest nor intend to have the sophistication of [etcd](https://etcd.io/) or similar projects.  

## What problem does it solve?  
This is especially build for inter-**Agent** communication on a local device (such as a laptop) or container environments that are isolated.  It is especially built for *parallel agents* that need to agree on state. 

This is a *platform primitive* designed for local use, and intended to be used in flexible ways that can adapt to many different processes that could work in concert with one another.

### Wait.  Why would agents need to agree on state?  

There are many reasons.  

Many things can be virtualized on a computer or used in ways that minimize concurrency concerns, like

- CPUs
- Networking
- Databases and instances

While this project does not intend to argue about the [semantics or potential workarounds](https://knowyourmeme.com/memes/ackchyually-actually-guy), some resources have inherently limited capacity or cannot meaningfully be shared simultaneously:

- An entire GPU's capacity.
- All the RAM on your computer.
- A monitor that shows information to a user (While multiple windows have been in computing for more than 30 years, the number of eyes available to humans remains bounded, as well as attention span)
- Audio output.
- Taking control of the mouse. (Hmmm...)
- Any process or series of processes that implies order of operations

### Wait.  Why can't i just ask my agent to do it for me? 

Be my guest. 

While there are other ways, including writing to a local filesystem, this project's goal is to provide a *highly performant, minimal* service that may be used in place of *token-implicit* methodology when there are simpler, solution-oriented ways to get to the same result *with better performance*.  This unlocks the ability for agents and other parallel processes to scale, without the waste associated with tooling churn.


# Quick start

Start the server. It listens on `127.0.0.1:7319` by default:

```sh
stow serve
```

Then store, read, list, and delete a value:

```sh
curl -X PUT --data-binary 'hello' http://127.0.0.1:7319/api/v1/set/greeting
curl http://127.0.0.1:7319/api/v1/get/greeting
curl http://127.0.0.1:7319/api/v1/keys
curl -X DELETE http://127.0.0.1:7319/api/v1/delete/greeting
```

Data lives in memory only and is lost when the server stops, unless you pass `--save-file <PATH>` to restore it on the next start (see [USAGE.md](USAGE.md)).

For more examples, including conditional writes, expiring leases, and the full API reference, see [USAGE.md](USAGE.md).

# Memory

`stow` has the capability to run many instances side by side, so it keeps memory small.  The key table shrinks after large deletes and expiry sweeps.

Measured on macOS arm64 with a release build (RSS from `ps`):

| State | RSS |
|-------|-----|
| Idle | about 2.3 MB |
| 10,000 keys with 1 KiB values | about 14 MB |

