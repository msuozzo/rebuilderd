rebuilderd-worker(1)

# NAME

rebuilderd-worker - Run rebuilds for rebuilderd

# SYNOPSIS

*rebuilderd-worker* [-n name] [-c /etc/rebuilderd-worker.conf] <cmd>

# DESCRIPTION

The rebuilderd-worker binary receives work from *rebuilderd*(1), executes the
correct rebuilder script and reports the results back. You can also ask it to
rebuild a specific package without reporting the results for debugging.

The *-n <name>* flag works for both *connect* and *build* and automatically
switches into the work directory at */var/lib/rebuilderd-worker/<name>*.

# CONNECT

*rebuilderd-worker* connect [endpoint]

Connect to a given rebuilder and ask for work. If you endpoint is specified one
is loaded from */etc/rebuilderd-worker.conf*, see *rebuilderd-worker.conf*(5).

# BUILD

*rebuilderd-worker* build [--stabilizer <path>] <distro> <url>

This is a small wrapper around the rebuilder scripts that are used by
rebuilderd-worker.

*--stabilizer* <path>
	If the rebuilt artifact is not bit-for-bit identical, pass both the
	upstream and the rebuilt artifact through this executable and compare them
	again. This overrides the stabilizer configured for the backend, see
	*rebuilderd-worker.conf*(5).

# SEE ALSO

*rebuilderd*(1), *rebuilderd-worker.conf*(5), *repro*(8).

# AUTHORS

rebuilderd was originally written by kpcyrd and is hosted at https://github.com/kpcyrd/rebuilderd
