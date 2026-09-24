rebuilderd-worker.conf(5)

# NAME

rebuilderd-worker.conf - *rebuilderd-worker* configuration files

# SYNOPSIS

/etc/rebuilderd-worker.conf

# DESCRIPTION

This file configures *rebuilder-worker connect* and specifies both the endpoint
and the signup_secret. Both are optional but the endpoint needs to be
explicitly provided by commandline if it's not configured in the config.

# OPTIONS

_endpoint=_
	The rebuilderd to connect to.

_signup_secret=_
	The server would either allowlist our key or require a signup secret.

_idle_delay=_
	Number of seconds to sleep when no work is available (defaults to 180 seconds).

## [build]

_timeout=_
	Set a timeout in seconds after which diffoscope is terminated (defaults to 24 hours).

_max_bytes=_
	Set a maximum diffoscope output limit in bytes (default: none).
	When reaching this limit the log is truncated but the rebuilder backend is
	*not* terminated.

_silent=_
	By default build output is forwarded to stdout/stderr.
	This can be disabled by settings this to true.

## [diffoscope]

_enabled=_
	Generate and attach diffs with diffoscope when rebuilding.

_args=_
	Pass additional arguments to diffoscope. Use wisely, some options might not work well.

_timeout=_
	Set a timeout in seconds after which diffoscope is terminated (defaults to 1 hour).

_max_bytes=_
	Set a maximum diffoscope output limit in bytes (default: none).
	When reaching this limit diffoscope is terminated and the output is truncated.

## [backend."<distro>"]

Configures the rebuilder backend for a distribution, this section can be
repeated for each distribution the worker should build.

_path=_
	Path to the rebuilder script. It's invoked with the build input as its
	only argument and is expected to write the rebuilt artifacts to the
	directory in the *REBUILDERD_OUTDIR* environment variable.

_stabilizer=_
	Path to an executable that writes a canonical form of an artifact (optional).
	This is meant for package formats that embed data a rebuilder can never
	reproduce, like the signature in Alpine's .apk files.

	The stabilizer is invoked as *<stabilizer> <in> <out>*, it needs to write
	the canonical form of <in> to <out> and exit with 0. It's only used if the
	rebuilt artifact is not bit-for-bit identical to the upstream artifact, in
	that case both artifacts are passed through the stabilizer and the results
	are compared again, subject to the build timeout.

	If they match, the artifact is still reported as GOOD, but the file name
	of the stabilizer is recorded in the build report and shown by the API.
	The in-toto attestation records the stabilized rebuilt artifact as its
	product and the build input, the upstream artifact, the unmodified rebuilt
	artifact and the stabilizer itself as its materials, so the stabilizer that
	has been used is pinned by its hash.

# EXAMPLE

```
## The rebuilderd to connect to
endpoint = "http://127.0.0.1:8484"
## The server would either allowlist our key or require a signup secret
#signup_secret = "INSECURE"

[build]
#timeout = 86400 # 24 hours
## Set a maximum build log limit in bytes (default: none).
## When reaching this limit the log is truncated but the rebuilder backend is *not* terminated.
max_bytes = 10485760 # 10 MiB

[diffoscope]
## Generate and attach diffs with diffoscope when rebuilding
enabled = false
## Pass additional arguments to diffoscope. Use wisely, some options might not work well.
#args = ["--max-container-depth", "2", "--fuzzy-threshold", "0"]
## Set a timeout in seconds after which diffoscope is terminated (default: 3600)
#timeout = 600 # 10 minutes
## Set a maximum diffoscope output limit in bytes (default: none).
## When reaching this limit diffoscope is terminated and the output is truncated.
max_bytes = 41943040 # 40 MiB

[backend."archlinux"]
path = "/usr/libexec/rebuilderd/rebuilder-archlinux.sh"

#[backend."alpine"]
#path = "/usr/libexec/rebuilderd/rebuilder-alpine.sh"
## If the rebuilt artifact is not bit-for-bit identical, pass both artifacts
## through this program and compare them again
#stabilizer = "/usr/libexec/rebuilderd/stabilizer-alpine.py"
```

# SEE ALSO

*rebuilderd-worker*(1).

# AUTHORS

rebuilderd was originally written by kpcyrd and is hosted at https://github.com/kpcyrd/rebuilderd
